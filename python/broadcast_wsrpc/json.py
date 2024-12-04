from typing import Sequence, TypeAlias, Mapping

JsonType: TypeAlias = Mapping[str, "JsonType"] | Sequence["JsonType"] | str | int | float | bool | None
JsonObject: TypeAlias = Mapping[str, JsonType]
JsonArray: TypeAlias = list[JsonType]