import asyncio
import json
from asyncio import Queue, QueueFull
from typing import Callable
from uuid import uuid4

import msgpack
from websockets import connect

RX_QUEUE_SIZE = 1000


class Receiver(object):
    """
    A receiver receives message objects from an RPC `Client`. It is typically obtained
    by calling `client.listen()` (or `client.notifications()`, `client.replies()`, ...).

    The user can listen to messages on the bus by using the `with` statement::

        with client.listen() as receiver:
            for _ in range(10):
                message = await receiver.next(timeout=0.1)
                print(message)

    If the receiver is not used as a context manager, it needs to be connected to the client by calling `receiver.connect()` first.
    Then, it needs to be unregistered once the application is done receiving messages (using `receiver.disconnect()`).
    Otherwise, messages will pile up in the internal buffer, until it is full.
    The receiver may be connected and disconnected several times.

    Receivers may contain a message filter map function. A filter may be applied
    by calling `receiver.map(...)`. Refer to the `receiver.map()` function for more details.
    """

    def __init__(self, client, flt):
        self._queue = Queue(RX_QUEUE_SIZE)
        self._client = client
        self._flt = flt
        self._connected = False

    @property
    def flt(self):
        """
        Return the filter-map function to be applied to messages
        """
        return self._flt

    @property
    def client(self):
        """
        Returns the underlying client
        """
        return self._client

    @property
    def queue(self) -> Queue:
        """
        Returns the buffer queue with the message posted by the `Client`
        """
        return self._queue

    async def next(self, timeout=None):
        """
        Receive the next message after applying all the filters but wait for at most `timeout` seconds.
        If `timeout` is None, wait forever.
        """
        if timeout is None:
            msg = await self._queue.get()
        else:
            msg = await asyncio.wait_for(self._queue.get(), timeout)
        if isinstance(msg, Exception):
            raise msg
        return msg

    def disconnect(self):
        """
        Disconnect from the client and stop listening to incoming messages
        """
        if self._connected:
            self._client._unregister(self)
            self._connected = False
        return self

    def connect(self):
        """
        Connect to the client and start listening to incoming messages
        """
        if not self._connected:
            self._client._register(self)
            self._connected = True
        return self

    def __enter__(self):
        return self.connect()

    def __exit__(self, exc_type, exc_val, exc_tb):
        self.disconnect()

    def get_all(self):
        """
        Retrieve all messages from the internal buffer without blocking.
        This should only be used for debugging purposes.
        """
        ret = []
        while True:
            try:
                rx = self._queue.get_nowait()
                self._queue.task_done()
                ret.append(rx)
            except:
                break
        return ret

    def clear_all(self):
        """
        Clear all messages in the internal buffer.
        """
        self.get_all()

    def map(self, fun: Callable):
        """
        Filter-map messages received by this receiver.

        `fun` is called for every message received on the bus and may return
        a modified object (`map()` step)
        If `fun` returns None, the message is discared and is not
        propageted to the user (`filter()` step).
        """
        flt = self._flt

        def new_fun(x):
            x = flt(x)
            if x is not None:
                return fun(x)
            return None

        return Receiver(self._client, new_fun)


class Client(object):
    """
    A async implementation of a websocket RPC client.
    Incoming messages are send to a `asyncio.Queue`, which is accessible
    over `Client.queue`.
    """

    def __init__(self):
        self._ws = None
        self._receivers = {}

    async def connect(self, url, **kw):
        """
        Connect to a remote server.
        Spawns a new task handling incoming messages.

        :param **kw: keyword arguments passed to `websockets.connect()`
        """
        if self._ws is not None:
            return
        self._ws = await connect(url, **kw)
        asyncio.create_task(self._rx_loop())
        return self

    @property
    def connected(self):
        """
        Returns True if the `Client` has been connected to the remote server.
        """
        return self._ws is not None

    async def _rx_loop(self):
        while True:
            try:
                msg = await self._ws.recv()
            except Exception:
                break
            try:
                if isinstance(msg, bytes):
                    msg = msgpack.unpackb(msg)
                else:
                    msg = json.loads(msg)
            except Exception:
                continue
            for receiver in self._receivers.values():
                try:
                    assert isinstance(receiver, Receiver)
                    mapped = receiver.flt(msg)
                    if mapped is not None:
                        try:
                            receiver.queue.put_nowait(mapped)
                        except QueueFull:
                            pass
                except Exception as e:
                    try:
                        receiver.queue.put_nowait(e)
                    except QueueFull:
                        pass
                    continue
        self._ws = None

    def listen(self, flt=None) -> Receiver:
        """
        Listen to messages on the buf, optionally applying the `flt` filter-map function.
        Refer to the documentation of the `Reciever` class for more information.
        """
        if flt is None:
            flt = lambda x: x
        rx = Receiver(self, flt)
        return rx

    def replies(self):
        """
        Return a `Receiver` filtering messages for `Reply` messages.
        """
        return self.listen(lambda x: x["Reply"]["message"] if "Reply" in x else None)

    def notifications(self):
        """
        Return a `Receiver` filtering messages for `Notification` messages.
        """
        return self.listen(lambda x: x["Notify"] if "Notify" in x else None)

    def messages(self):
        """
        Return a `Receiver` filtering messages for `Notification` and `Reply` messages.
        """
        def mapper(x):
            if "Reply" in x:
                return x["Reply"]["message"]
            elif "Notify" in x:
                return x["Notify"]
            return None

        return self.listen(mapper)

    def _register(self, rx: Receiver):
        self._receivers[id(rx)] = rx

    def _unregister(self, rx: Receiver):
        key = id(rx)
        if key in self._receivers:
            del self._receivers[id(rx)]

    async def send_request(self, msg, id=None) -> str:
        """
        Send a request, returning the request id.
        """
        if id is None:
            id = str(uuid4())
        await self._ws.send(json.dumps({"id": id, "message": msg}))
        return id

    async def request(self, msg, timeout=1.0):
        """
        Send a request and wait for the answer.
        Returns None if no answer was received during the timeout.
        """
        id = str(uuid4())

        def flt(msg):
            ret = "Reply" in msg and msg["Reply"]["request"] == id
            if ret:
                return msg["Reply"]["message"]
            return None

        with self.listen(flt) as rx:
            await self.send_request(msg, id=id)
            return await asyncio.wait_for(rx.next(), timeout)

    async def disconnect(self):
        """
        Close the websocket connection.
        """
        if self._ws is None:
            return
        await self._ws.close()
        self._ws = None
