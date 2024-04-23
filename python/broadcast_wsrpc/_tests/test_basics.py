import asyncio

from broadcast_wsrpc import JsonType
from broadcast_wsrpc.client import Client
from broadcast_wsrpc.server import Server
import pytest


@pytest.mark.asyncio
async def test_listener() -> None:
    async def handler(srv: Server, req: str) -> JsonType:
        if req == "close":
            await srv.shutdown()
        return req

    server = Server(handler)
    server_task = asyncio.create_task(server.run("127.0.0.1", 1234))
    await asyncio.sleep(0.1)

    client = await Client().connect("ws://127.0.0.1:1234")
    listener = (
        client.listen(lambda x: x["Reply"]["message"] if "Reply" in x else None)  # type: ignore
        .map(lambda x: x["foo"] if "foo" in x else None)  # type: ignore
        .map(lambda x: x["bar"] if "bar" in x else None)  # type: ignore
    )

    with listener as rx:
        await client.send_request({"foo": {}})
        try:
            _ = await rx.next(0.1)
            assert False
        except Exception:
            pass
        await client.send_request({"foo": {"bar": 123}})
        msg = await rx.next(0.1)
        assert msg == 123
        await client.send_request("close")
        await server_task
