import asyncio
import json
from asyncio import Queue, QueueFull
from uuid import uuid4

import msgpack
from websockets import connect

RX_QUEUE_SIZE = 1000


class ReceiverDisconnected(Exception):
    pass


class Receiver(object):
    def __init__(self, client):
        self._queue = Queue(RX_QUEUE_SIZE)
        self._client = client
        self._connected = True

    @property
    def client(self):
        return self._client

    @property
    def queue(self) -> Queue:
        return self._queue

    async def next(self):
        return await self._queue.get()

    def disconnect(self):
        try:
            self._client._unregister(self)
        except ReceiverDisconnected:
            pass
        self._connected = False

    def __enter__(self):
        if not self._connected:
            raise ReceiverDisconnected()
        return self

    def __exit__(self, exc_type, exc_val, exc_tb):
        self.disconnect()

    def get_all(self):
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
        self.get_all()


class Client(object):
    """
    A async implementation of a websocket RPC client.
    Incoming messages are send to a `asyncio.Queue`, which is accessible
    over `Client.queue`.
    """

    def __init__(self):
        self._ws = None
        self._receivers = {}

    async def connect(self, url):
        """
        Connect to a remote server.
        Spawns a new task handling incoming messages.
        """
        if self._ws is not None:
            return
        self._ws = await connect(url)
        asyncio.create_task(self._rx_loop())
        return self

    @property
    def connected(self):
        return self._ws is not None

    async def _rx_loop(self):
        while True:
            msg = await self._ws.recv()
            try:
                if isinstance(msg, bytes):
                    msg = msgpack.unpackb(msg)
                else:
                    msg = json.loads(msg)
                for (flt, receiver) in self._receivers.values():
                    assert isinstance(receiver, Receiver)
                    mapped = flt(msg)
                    if mapped is not None:
                        try:
                            receiver.queue.put_nowait(mapped)
                        except QueueFull:
                            pass
            except:
                continue
        self._ws = None

    def listen(self, flt=lambda msg: msg) -> Receiver:
        rx = Receiver(self)
        self._receivers[id(rx)] = (flt, rx)
        return rx

    def _unregister(self, rx: Receiver):
        key = id(rx)
        if key not in self._receivers:
            raise ReceiverDisconnected
        del self._receivers[id(rx)]

    async def send_request(self, msg, id=None) -> str:
        """
        Send a request, returning the request id.
        """
        if id is None:
            id = str(uuid4())
        await self._ws.send(json.dumps({'id': id, 'message': msg}))
        return id

    async def query(self, msg, timeout=1.0):
        """
        Send a request and wait for the answer.
        Returns None if no answer was received during the timeout.
        """
        id = str(uuid4())

        def flt(msg):
            ret = 'Reply' in msg and msg['Reply']['request'] == id
            if ret:
                return msg['Reply']['message']
            return None

        rx = self.listen(flt)
        await self.send_request(msg, id=id)
        try:
            return await asyncio.wait_for(rx.next(), timeout)
        except TimeoutError:
            pass
        return None

    async def disconnect(self):
        await self._ws.close()
