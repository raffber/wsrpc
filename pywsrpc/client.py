import asyncio
import json
from asyncio import Queue
from datetime import datetime
from uuid import uuid4

import msgpack
from websockets import connect


class Client(object):
    """
    A async implementation of a websocket RPC client.
    Incoming messages are send to a `asyncio.Queue`, which is accessible
    over `Client.queue`.
    """
    def __init__(self):
        self._queue = Queue()
        self._ws = None

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
                await self._queue.put(msg)
            except:
                continue
        self._ws = None

    async def send_request(self, msg) -> str:
        """
        Send a request, returning the request id.
        """
        id = str(uuid4())
        await self._ws.send(json.dumps({'id': id, 'message': msg}))
        return id

    async def query(self, msg, timeout=1.0):
        """
        Send a request and wait for the answer.
        Returns None if no answer was received during the timeout.
        """
        id = await self.send_request(msg)
        start = datetime.now()
        while True:
            msg = await asyncio.wait_for(self._queue.get(), timeout)
            if 'Reply' in msg:
                rx_id = msg['Reply']['request']
                if rx_id == id:
                    return msg['Reply']['message']
            now = datetime.now()
            if (now - start).total_seconds() > timeout:
                break
        return None

    @property
    def queue(self):
        """
        Provides access to the internal queue of incoming messages.
        """
        return self._queue
