"""Emitter-facing ABI 3 support; applications should not import this module."""

from __future__ import annotations

from .envelope import Response
from .errors import BridgeErrorRegistry
from .types import (
    bool_from_wire,
    bounded_int_from_wire,
    buffer_from_wire,
    bytes_from_wire,
    enum_from_wire,
    float_from_wire,
    i64_from_wire,
    i64_to_wire,
    json_from_wire,
    json_to_wire,
    list_from_wire,
    map_from_wire,
    null_from_wire,
    string_from_wire,
    tuple_from_wire,
    u64_from_wire,
    u64_to_wire,
)

__all__ = [
    "EMITTER_API_VERSION",
    "BridgeErrorRegistry",
    "Response",
    "bool_from_wire",
    "bounded_int_from_wire",
    "buffer_from_wire",
    "bytes_from_wire",
    "enum_from_wire",
    "float_from_wire",
    "i64_from_wire",
    "i64_to_wire",
    "json_from_wire",
    "json_to_wire",
    "list_from_wire",
    "map_from_wire",
    "null_from_wire",
    "require_emitter_api",
    "string_from_wire",
    "tuple_from_wire",
    "u64_from_wire",
    "u64_to_wire",
]

# Generated clients explicitly pin this contract. It is independent of the
# native ABI version and must change whenever helper signatures change.
EMITTER_API_VERSION = 3


def require_emitter_api(version: int) -> None:
    """Fail early when generated code and its installed runtime disagree."""
    if version != EMITTER_API_VERSION:
        raise RuntimeError(
            f"rspyts: generated client requires emitter API {version}, "
            f"but this runtime provides {EMITTER_API_VERSION}; regenerate or install the matching runtime"
        )
