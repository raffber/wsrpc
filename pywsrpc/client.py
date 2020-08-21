import asyncio
import json
from asyncio import Queue
from datetime import datetime
from uuid import uuid4

import msgpack
from websockets import connect


class Client(object):
    def __init__(self):
        self._queue = Queue()
        self._ws = None

    async def connect(self, url):
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
        id = str(uuid4())
        await self._ws.send(json.dumps({'id': id, 'message': msg}))
        return id

    async def query(self, msg, timeout=1.0):
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
        return self._queue
