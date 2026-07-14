"""
rspyts — the Python runtime for Rust crates bridged with rspyts.

Notes:
    Define types, functions, and classes once in Rust; ``rspyts generate``
    emits a fully typed Python package whose models subclass
    :class:`Contract` and whose calls go through :class:`Library` across a
    small C ABI. This package is the runtime those generated packages
    import; it contains no generated code itself.

    Public surface (frozen by ``docs/design/codegen.md`` §4.1 — generated
    code imports exactly these names):

    - :class:`Contract` — pydantic base model with camelCase aliases and
      unknown-field rejection.
    - :class:`BridgeError` — base exception carrying
      ``code``/``message``/``data``.
    - :class:`RspytsPanicError` — a Rust panic crossed the boundary.
    - :class:`StaleHandleError` — a method call on a dropped handle.
    - :class:`Library` — locates, loads, and calls the compiled cdylib.
    - :data:`I64` / :data:`U64` — exact integer pydantic aliases.
    - :data:`JsonValue` — schemaless JSON with collision-safe wire encoding.
    - :func:`register_error` — optional process-wide error-code mapping for
      handwritten integrations; generated clients use call-scoped maps.

    The ``contract``, ``envelope``, ``errors``, ``library``, and ``types`` modules
    are re-exported for dotted access.
"""

from . import contract, envelope, errors, library, types
from .contract import Contract
from .errors import BridgeError, BridgeErrorRegistry, RspytsPanicError, StaleHandleError, register_error
from .library import Library
from .types import (
    I64,
    U64,
    JsonValue,
    float_from_wire,
    i64_from_wire,
    i64_to_wire,
    json_from_wire,
    json_to_wire,
    u64_from_wire,
    u64_to_wire,
)

__all__ = [
    "I64",
    "U64",
    "BridgeError",
    "BridgeErrorRegistry",
    "Contract",
    "JsonValue",
    "Library",
    "RspytsPanicError",
    "StaleHandleError",
    "contract",
    "envelope",
    "errors",
    "float_from_wire",
    "i64_from_wire",
    "i64_to_wire",
    "json_from_wire",
    "json_to_wire",
    "library",
    "register_error",
    "types",
    "u64_from_wire",
    "u64_to_wire",
]

__version__ = "0.2.0"
