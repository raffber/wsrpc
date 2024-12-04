from typing import TypeAlias

JsonType: TypeAlias = dict[str, "JsonType"] | list["JsonType"] | str | int | float | bool | None
JsonObject: TypeAlias = dict[str, JsonType]
JsonArray: TypeAlias = list[JsonType]