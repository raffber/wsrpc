import asyncio

from broadcast_wsrpc.client import Client
from broadcast_wsrpc.server import Server
import pytest


@pytest.mark.asyncio
async def test_listener():
    async def handler(srv, req):
        if req == 'close':
            await srv.shutdown()
        return req

    server = Server(handler)
    server_task = asyncio.create_task(server.run('127.0.0.1', 1234))
    await asyncio.sleep(0.1)

    client = await Client().connect('ws://127.0.0.1:1234')

    with client.listen(lambda x: x['Reply']['message'] if 'Reply' in x else None).map(
            lambda x: x['foo'] if 'foo' in x else None).map(
            lambda x: x['bar'] if 'bar' in x else None) as rx:
        await client.send_request({'foo': {}})
        try:
            _ = await rx.next(0.1)
            assert False
        except:
            pass
        await client.send_request({'foo': {'bar': 123}})
        msg = await rx.next(0.1)
        assert msg == 123
        await client.send_request('close')
        await server_task
