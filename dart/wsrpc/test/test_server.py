import sys
from os.path import abspath, dirname
import os
import asyncio

curdir = dirname(abspath(__file__))
rootdir = abspath(os.path.join(curdir, '..', '..', '..'))

sys.path.insert(0, rootdir)

from pywsrpc.server import Server


async def handler(server: Server, message: dict):
    pass

async def main():
    pass


if __name__ == '__main__':
    asyncio.run(main())


