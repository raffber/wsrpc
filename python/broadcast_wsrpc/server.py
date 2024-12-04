import asyncio
import json
from asyncio import Task, iscoroutinefunction
from typing import Any, Awaitable, Callable, Dict
from uuid import UUID

from broadcast_wsrpc import JsonType
import msgpack  # type: ignore
from websockets import WebSocketServer, WebSocketServerProtocol, serve, ConnectionClosed


class Quit(Exception):
    """
    Exception to quit the server task
    """

    pass


class InvalidRequest(Exception):
    """
    This exception may be raised in the request handler functionpassed with `Server(handler)`.
    It should be used in the case the request was invalid.
    """

    def __init__(self, message: str) -> None:
        self._message = message
        super().__init__(message)

    @property
    def message(self) -> str:
        return self._message


REQUEST_SCHEMA = {
    "type": "object",
    "id": {"type": "string"},
    "message": {"type": "object"},
}

HandlerType = Callable[["Server", JsonType], JsonType | Awaitable[JsonType]]


class Server(object):
    """
    Async websocket RPC server implementation.

    The server must be given a message handler, which is called for
    each incoming message. The handler may return a message, in which
    case the message is sent back onto the "bus".
    """

    def __init__(self, handler: HandlerType):
        self._connected: Dict[Connection, Task[None]] = {}
        self._server: WebSocketServer | None = None
        self._handler = handler

    async def run(self, socketaddr: str, port: int, **kw: Any) -> None:
        """
        Creates a websocket server on the given address, listening on the given port.
        This function will only return, once the server has terminated.

        :param **kw: keyword arguments passed to `websockets.serve()`
        """
        self._server = await serve(self._connection_handler, socketaddr, port, **kw)
        await self._server.wait_closed()

    async def broadcast(self, msg: JsonType) -> None:
        """
        Broadcast a message to all clients
        """
        msg = {"Notify": msg}
        connected = list(self._connected.keys())
        msg = json.dumps(msg)
        for con in connected:
            await con.send(msg)

    async def answer(self, id: str, msg: JsonType) -> None:
        """
        Answer a previous request.
        """
        msg = {"Reply": {"request": id, "message": msg}}
        connected = list(self._connected.keys())
        msg = json.dumps(msg)
        for con in connected:
            await con.send(msg)

    async def send_invalid_request(self, id: str, msg: JsonType) -> None:
        msg = {"InvalidRequest": {"id": id, "description": msg}}
        connected = list(self._connected.keys())
        msg = json.dumps(msg)
        for con in connected:
            await con.send(msg)

    async def send_error(self, msg: JsonType) -> None:
        """
        Send an error message
        """
        msg = {"Error": msg}
        connected = list(self._connected.keys())
        msg = json.dumps(msg)
        for con in connected:
            await con.send(msg)

    async def _connection_handler(self, ws: WebSocketServerProtocol, path: str) -> None:
        connection = Connection(self, ws)
        connection_task = asyncio.create_task(connection.run())
        self._connected[connection] = connection_task
        await connection_task

    def unregister_client(self, connection: "Connection") -> None:
        del self._connected[connection]

    async def shutdown(self) -> None:
        """
        Close all connections and shut down the server
        """
        for connection, _ in list(self._connected.items()):
            await connection.close()
        if self._server is not None:
            self._server.close()

    @property
    def handler(self) -> HandlerType:
        return self._handler


class Connection(object):
    """
    Handles a client connection
    """

    def __init__(self, server: Server, ws: WebSocketServerProtocol) -> None:
        self._server = server
        self._ws = ws

    async def close(self) -> None:
        """
        Close this connection
        """
        await self._ws.close()

    async def run(self) -> None:
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

    async def send(self, msg: str | bytes) -> None:
        """
        Send a message to this connection
        """
        await self._ws.send(msg)

    async def _loop(self) -> None:
        # handles one incoming message
        msg = await self._ws.recv()
        try:
            if isinstance(msg, str):
                msg = json.loads(msg)
            else:  # isinstance(msg, bytes)
                msg = msgpack.unpackb(msg) # type: ignore
        except Exception:
            await self._server.send_error("Invalid JSON")
            return
        if "id" not in msg:
            # not a request
            return
        if not isinstance(msg["id"], str):  # type: ignore
            await self._server.send_error("Invalid UUID")
            return
        id = msg["id"]  # type: ignore
        if not isinstance(id, str):
            await self._server.send_error("Invalid UUID")
            return
        try:
            _ = UUID(id) # type: ignore
        except ValueError:
            await self._server.send_error("Invalid UUID")
            return
        # call back into the application
        handler = self._server.handler
        try:
            content: JsonType = msg["message"] # type: ignore
            if iscoroutinefunction(handler):
                reply: JsonType = await handler(self._server, content) # type: ignore
            else:
                reply: JsonType = handler(self._server, content) # type: ignore
        except InvalidRequest as e:
            await self._server.send_invalid_request(id, e.message)
            return
        if reply is not None:
            await self._server.answer(id, reply)
