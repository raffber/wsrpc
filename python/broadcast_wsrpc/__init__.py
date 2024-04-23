from typing import Any, Dict, List

JsonDict = Dict[str, Any]
JsonList = List[Any]
JsonType = None | int | str | bool | JsonList | JsonDict

from .client import Client  # noqa
from .server import Server  # noqa


__all__ = ["Client", "Server", "JsonDict", "JsonList", "JsonType"]
