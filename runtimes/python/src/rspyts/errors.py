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
]


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


def raise_bridge_error(
    status: int,
    payload: object,
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
    if status not in (1, 2):
        raise ValueError(f"rspyts: cannot raise a bridge error for response status {status}")
    if type(payload) is not dict:
        raise ValueError(f"rspyts: error payload must be a JSON object, got {type(payload).__name__}")
    fields = typing.cast(dict[object, object], payload)
    allowed = {"code", "message", "data"}
    if "code" not in fields or "message" not in fields:
        raise ValueError("rspyts: error payload requires exact 'code' and 'message' fields")
    unknown = set(fields) - allowed
    if unknown:
        raise ValueError(f"rspyts: error payload has unexpected fields: {sorted(unknown)!r}")
    code = fields["code"]
    message = fields["message"]
    if type(code) is not str or not code:
        raise ValueError("rspyts: error payload 'code' must be a non-empty JSON string")
    if type(message) is not str:
        raise ValueError("rspyts: error payload 'message' must be a JSON string")
    data = fields.get("data")
    if status == 2:
        if code != "panic":
            raise ValueError(f"rspyts: panic payload must use code 'panic', got {code!r}")
        raise RspytsPanicError(message, code=code, data=data)
    cls = (error_types or {}).get(code)
    if cls is None:
        cls = StaleHandleError if code == "staleHandle" else BridgeError
    if not (isinstance(cls, type) and issubclass(cls, BridgeError)):
        raise TypeError(f"rspyts: error registry entry for {code!r} is not a BridgeError subclass")
    raise cls(message, code=code, data=data)
