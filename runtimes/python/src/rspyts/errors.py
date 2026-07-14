"""
Errors that cross the Rust boundary.
"""

from __future__ import annotations

import collections.abc
import typing

__all__ = [
    "BridgeError",
    "BridgeErrorRegistry",
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
    data: typing.Any | None

    def __init__(self, message: str, *, code: str, data: typing.Any | None = None) -> None:
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

    def __init__(self, message: str, *, code: str = "panic", data: typing.Any | None = None) -> None:
        super().__init__(message, code=code, data=data)


class StaleHandleError(BridgeError):
    """
    A method was called on a dropped or unknown handle (ABI §8).
    """

    def __init__(self, message: str, *, code: str = "staleHandle", data: typing.Any | None = None) -> None:
        super().__init__(message, code=code, data=data)


# Call-scoped map from wire error codes to generated exception classes.
BridgeErrorRegistry: typing.TypeAlias = collections.abc.Mapping[str, type[BridgeError]]


def register_error(code: str, cls: type[BridgeError]) -> None:
    """
    Map an error ``code`` to the exception class raised for it.

    Notes:
        This process-wide compatibility hook is useful for handwritten
        integrations. Generated clients use call-scoped maps so unrelated
        packages may safely reuse the same error code. Later global
        registrations for the same code win.

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


def raise_bridge_error(
    status: int,
    payload: dict[str, typing.Any],
    error_types: BridgeErrorRegistry | None = None,
) -> typing.NoReturn:
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
    cls = (error_types or {}).get(payload["code"]) or REGISTRY.get(payload["code"], BridgeError)
    raise cls(payload["message"], code=payload["code"], data=payload.get("data"))
