import json
from asyncio import Queue
import asyncio
from uuid import UUID

import msgpack
from jsonschema import validate, ValidationError
from websockets import serve, ConnectionClosed

from poke.server import Quit

REQUEST_SCHEMA = {
    "type": "object",
    "id": {"type": "string"},
    "message": {"type": "object"},
}


class Server(object):
    def __init__(self, handler):
        self._connected = {}
        self._server = None
        self._handler = handler

    async def run(self, socketaddr: str, port: int):
        self._server = await serve(self._connection_handler, socketaddr, port)
        await self._server.wait_closed()

    async def broadcast(self, msg):
        msg = {'Notify': msg}
        connected = list(self._connected.keys())
        msg = json.dumps(msg)
        for con in connected:
            await con.send(msg)

    async def answer(self, id, msg):
        msg = {'Reply': {'request': id, 'message': msg}}
        connected = list(self._connected.keys())
        msg = json.dumps(msg)
        for con in connected:
            await con.send(msg)

    async def send_error(self, msg):
        msg = {"Error": msg}
        connected = list(self._connected.keys())
        msg = json.dumps(msg)
        for con in connected:
            await con.send(msg)

    async def _connection_handler(self, ws, path):
        connection = Connection(self, ws)
        connection_task = asyncio.create_task(connection.run())
        self._connected[connection] = connection_task
        await connection_task

    def unregister_client(self, client):
        del self._connected[client]

    async def shutdown(self):
        for connection, task in self._connected.items():
            await connection.close()
            # task.cancel()
        self._server.close()

    @property
    def handler(self):
        return self._handler


class Connection(object):
    def __init__(self, server: Server, ws):
        self._server = server
        self._ws = ws

    async def close(self):
        await self._ws.close()

    async def run(self):
        try:
            while True:
                await self._loop()
        except Quit:
            await self._server.shutdown()
        except ConnectionClosed:
            pass
        finally:
            self._server.unregister_client(self)

    async def send(self, msg):
        await self._ws.send(msg)

    async def _loop(self):
        msg = await self._ws.recv()
        try:
            if isinstance(msg, str):
                msg = json.loads(msg)
            else:  # isinstance(msg, bytes)
                msg = msgpack.unpackb(msg)
        except:
            await self._server.send_error("Invalid JSON")
            return
        if 'id' not in msg:
            # not a request
            return
        if not isinstance(msg['id'], str):
            await self._server.send_error("Invalid UUID")
            return
        id = msg['id']
        try:
            UUID(id)
        except ValueError:
            await self._server.send_error("Invalid UUID")
            return
        reply = self._server.handler(msg['message'])
        if reply is not None:
            await self._server.answer(id, reply)
