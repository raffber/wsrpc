import asyncio
import json
from asyncio import Queue, QueueFull
from datetime import datetime
from uuid import uuid4

import msgpack
from websockets import connect

RX_QUEUE_SIZE = 1000


class Receiver(object):
    def __init__(self, client):
        self._queue = Queue(RX_QUEUE_SIZE)
        self._client = client

    @property
    def client(self):
        return self._client

    @property
    def queue(self) -> Queue:
        return self._queue

    async def next(self):
        return await self._queue.get()

    def disconnect(self):
        self._client.unregister(self)


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
            raise ValueError('Already started')
        self._ws = await connect(url)
        asyncio.create_task(self._rx_loop())

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
                    if flt is None or flt(msg):
                        try:
                            receiver.queue.put_nowait(msg)
                        except QueueFull:
                            pass
            except:
                continue
        self._ws = None

    def listen(self, flt=None) -> Receiver:
        rx = Receiver(self)
        self._receivers[id(rx)] = (flt, rx)
        return rx

    def unregister(self, rx: Receiver):
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
            return 'Reply' in msg and msg['Reply']['request'] == id

        rx = self.listen(flt)
        await self.send_request(msg)
        try:
            return await asyncio.wait_for(rx.next(), timeout)
        except TimeoutError:
            pass
        return None
