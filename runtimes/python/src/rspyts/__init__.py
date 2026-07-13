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

    - :class:`Contract` — pydantic base model (camelCase aliases, strict).
    - :class:`BridgeError` — base exception carrying
      ``code``/``message``/``data``.
    - :class:`RspytsPanicError` — a Rust panic crossed the boundary.
    - :class:`StaleHandleError` — a method call on a dropped handle.
    - :class:`Library` — locates, loads, and calls the compiled cdylib.
    - :func:`register_error` — maps error codes to generated exception
      classes.

    The ``contract``, ``envelope``, ``errors``, and ``library`` modules
    are re-exported for dotted access.
"""

from . import contract, envelope, errors, library
from .contract import Contract
from .errors import BridgeError, RspytsPanicError, StaleHandleError, register_error
from .library import Library

__all__ = [
    "BridgeError",
    "Contract",
    "Library",
    "RspytsPanicError",
    "StaleHandleError",
    "contract",
    "envelope",
    "errors",
    "library",
    "register_error",
]

__version__ = "0.1.0"
