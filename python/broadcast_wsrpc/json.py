from typing import Sequence, TypeAlias, Mapping

JsonObject: TypeAlias = Mapping[str, "JsonType"]
JsonArray: TypeAlias = Sequence["JsonType"]
JsonType: TypeAlias = JsonObject | JsonArray | str | int | float | bool | None