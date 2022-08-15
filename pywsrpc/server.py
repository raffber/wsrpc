import asyncio
import json
from asyncio import iscoroutinefunction
from uuid import UUID

import msgpack
from websockets import serve, ConnectionClosed


class Quit(Exception):
    """
    Exception to quit the server task
    """
    pass


class InvalidRequest(Exception):
    def __init__(self, message: str) -> None:
        self._message = message
        super().__init__(message)

    @property
    def message(self):
        return self._message


REQUEST_SCHEMA = {
    "type": "object",
    "id": {"type": "string"},
    "message": {"type": "object"},
}


class Server(object):
    """
    Async websocket RPC server implementation.

    The server must be given a message handler, which is called for
    each incoming message. The handler may return a message, in which
    case the message is sent back onto the "bus".
    The signature of the handler is: `def handler(Server, dict)`
    """
    def __init__(self, handler):
        self._connected = {}
        self._server = None
        self._handler = handler

    async def run(self, socketaddr: str, port: int, **kw):
        """
        Creates a websocket server on the given address, listening on the given port.
        This function will only return, once the server has terminated.

        :param **kw: keyword arguments passed to `websockets.serve()`
        """
        self._server = await serve(self._connection_handler, socketaddr, port, **kw)
        await self._server.wait_closed()

    async def broadcast(self, msg):
        """
        Broadcast a message to all clients
        """
        msg = {'Notify': msg}
        connected = list(self._connected.keys())
        msg = json.dumps(msg)
        for con in connected:
            await con.send(msg)

    async def answer(self, id, msg):
        """
        Answer a previous request.
        """
        msg = {'Reply': {'request': id, 'message': msg}}
        connected = list(self._connected.keys())
        msg = json.dumps(msg)
        for con in connected:
            await con.send(msg)

    async def send_invalid_request(self, id, msg):
        msg = {'InvalidRequest': {'id': id, 'description': msg}}
        connected = list(self._connected.keys())
        msg = json.dumps(msg)
        for con in connected:
            await con.send(msg)

    async def send_error(self, msg):
        """
        Send an error message
        """
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
        """
        Close all connections and shut down the server
        """
        for connection, task in list(self._connected.items()):
            await connection.close()
            # task.cancel()
        self._server.close()

    @property
    def handler(self):
        return self._handler


class Connection(object):
    """
    Handles a client connection
    """
    def __init__(self, server: Server, ws):
        self._server = server
        self._ws = ws

    async def close(self):
        """
        Close this connection
        """
        await self._ws.close()

    async def run(self):
        """
        Run this connection. This coroutine
        will run until this connection has been
        closed.
        """
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
        """
        Send a message to this connection
        """
        await self._ws.send(msg)

    async def _loop(self):
        # handles one incoming message
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
        # call back into the application
        handler = self._server.handler
        try:
            if iscoroutinefunction(handler):
                reply = await handler(self._server, msg['message'])
            else:
                reply = handler(self._server, msg['message'])
        except InvalidRequest as e:
            await self._server.send_invalid_request(id, e.message)
            return
        if reply is not None:
            await self._server.answer(id, reply)
