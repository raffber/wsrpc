from typing import Any, Dict, List
from .client import Client
from .server import Server


__all__ = ["Client", "Server"]

JsonDict = Dict[str, Any]
JsonList = List[Any]
JsonType = None | int | str | bool | JsonList | JsonDict
