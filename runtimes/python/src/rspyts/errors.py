"""
The bridge error model on the Python side (ABI §5).

Notes:
    A bridged function that fails returns an envelope whose JSON payload is
    ``{"code": …, "message": …, "data": …}``. This module turns those
    payloads into exceptions:

    - status 1 -> the class registered for ``code`` (generated
      ``errors.py`` modules register one subclass per error code at import
      time), falling back to plain :class:`BridgeError`;
    - status 2 -> :class:`RspytsPanicError`, always.
"""

from __future__ import annotations

from typing import Any, NoReturn

__all__ = [
    "BridgeError",
    "RspytsPanicError",
    "StaleHandleError",
    "raise_bridge_error",
    "register_error",
]

REGISTRY: dict[str, type[BridgeError]] = {}


class BridgeError(Exception):
    """
    An error that crossed the Rust boundary.

    Notes:
        Carries the machine-readable ``code``, the human-readable
        ``message``, and optional structured ``data`` (the named fields of
        the Rust error variant).
    """

    code: str
    message: str
    data: Any | None

    def __init__(self, message: str, *, code: str, data: Any | None = None) -> None:
        self.code = code
        self.message = message
        self.data = data
        super().__init__(f"[{code}] {message}")


class RspytsPanicError(BridgeError):
    """
    A panic crossed the shim boundary (envelope status 2).

    Notes:
        Indicates a bug in the bridged Rust code, not a domain error;
        callers should generally not catch it.
    """

    def __init__(self, message: str, *, code: str = "panic", data: Any | None = None) -> None:
        super().__init__(message, code=code, data=data)


class StaleHandleError(BridgeError):
    """
    A method was called on a dropped or unknown handle (ABI §8).
    """

    def __init__(self, message: str, *, code: str = "staleHandle", data: Any | None = None) -> None:
        super().__init__(message, code=code, data=data)


def register_error(code: str, cls: type[BridgeError]) -> None:
    """
    Map an error ``code`` to the exception class raised for it.

    Notes:
        Generated ``errors.py`` modules call this at import time for every
        error variant, so callers can catch exact per-code subclasses.
        Later registrations for the same code win.

    Args:
        code: The wire error code.
        cls: The exception class to raise for that code.

    Raises:
        TypeError: If ``cls`` is not a :class:`BridgeError` subclass.
    """
    if not (isinstance(cls, type) and issubclass(cls, BridgeError)):
        raise TypeError(f"register_error requires a BridgeError subclass, got {cls!r}")
    REGISTRY[code] = cls


register_error("staleHandle", StaleHandleError)


def raise_bridge_error(status: int, payload: dict[str, Any]) -> NoReturn:
    """
    Raise the exception for a non-ok envelope (status 1 or 2).

    Args:
        status: The envelope status byte.
        payload: The decoded error payload.

    Raises:
        BridgeError: The registered subclass for ``payload["code"]``
            (status 1), or :class:`RspytsPanicError` (status 2).
    """
    if status == 2:
        raise RspytsPanicError(payload["message"], code=payload["code"], data=payload.get("data"))
    cls = REGISTRY.get(payload["code"], BridgeError)
    raise cls(payload["message"], code=payload["code"], data=payload.get("data"))
