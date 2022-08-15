import sys
from os.path import abspath, dirname
import os
import asyncio

curdir = dirname(abspath(__file__))
rootdir = abspath(os.path.join(curdir, '..', '..', '..'))

sys.path.insert(0, rootdir)

from pywsrpc.server import Server, Quit


async def handler(server: Server, message: dict):
    print('Message received: {}'.format(message))
    if message == {'Foo': 'Bar'}:
        print('Match. Replying...')
        return message
    elif message == {'Quit': None}:
        raise Quit()


async def main():
    await Server(handler=handler).run('127.0.0.1', 7479)


if __name__ == '__main__':
    asyncio.run(main())

