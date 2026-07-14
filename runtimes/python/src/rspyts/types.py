"""
Portable host representations for exact rspyts scalar types.
"""

from __future__ import annotations

import math
import typing

import pydantic

__all__ = [
    "I64",
    "U64",
    "JsonValue",
    "float_from_wire",
    "i64_from_wire",
    "i64_to_wire",
    "json_from_wire",
    "json_to_wire",
    "u64_from_wire",
    "u64_to_wire",
]

I64_MIN = -(2**63)
I64_MAX = 2**63 - 1
U64_MAX = 2**64 - 1


def exact_int(value: object, *, signed: bool, allow_host_int: bool) -> int:
    """
    Validate one exact integer representation.

    Args:
        value: Host integer or canonical decimal string.
        signed: Whether to apply the signed range.
        allow_host_int: Whether a host integer is accepted.

    Returns:
        The validated integer.
    """
    label = "signed" if signed else "unsigned"
    if isinstance(value, bool):
        raise TypeError(f"expected a {label} 64-bit integer, got bool")
    if allow_host_int and isinstance(value, int):
        parsed = value
    elif isinstance(value, str):
        try:
            parsed = int(value, 10)
        except ValueError as error:
            raise ValueError(f"expected a canonical {label} 64-bit decimal string") from error
        if str(parsed) != value:
            raise ValueError(f"expected a canonical {label} 64-bit decimal string")
    else:
        raise TypeError(f"expected a {label} 64-bit integer, got {type(value).__name__}")

    minimum = I64_MIN if signed else 0
    maximum = I64_MAX if signed else U64_MAX
    if not minimum <= parsed <= maximum:
        kind = "i64" if signed else "u64"
        raise ValueError(f"{kind} value out of range: {parsed}")
    return parsed


def i64_from_wire(value: object) -> int:
    """
    Validate a canonical signed wire string.
    """
    return exact_int(value, signed=True, allow_host_int=False)


def u64_from_wire(value: object) -> int:
    """
    Validate a canonical unsigned wire string.
    """
    return exact_int(value, signed=False, allow_host_int=False)


def i64_to_wire(value: object) -> str:
    """
    Encode a Python integer as a canonical signed wire string.
    """
    return str(exact_int(value, signed=True, allow_host_int=True))


def u64_to_wire(value: object) -> str:
    """
    Encode a Python integer as a canonical unsigned wire string.
    """
    return str(exact_int(value, signed=False, allow_host_int=True))


def i64_host(value: object) -> int:
    """
    Validate a signed host integer for pydantic.
    """
    return exact_int(value, signed=True, allow_host_int=True)


def u64_host(value: object) -> int:
    """
    Validate an unsigned host integer for pydantic.
    """
    return exact_int(value, signed=False, allow_host_int=True)


def float_from_wire(value: object) -> float:
    """
    Validate a finite structured float and canonicalize signed zero.
    """
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise TypeError(f"expected a finite JSON number, got {type(value).__name__}")
    parsed = float(value)
    if not math.isfinite(parsed):
        raise ValueError("expected a finite JSON number")
    return 0.0 if parsed == 0 else parsed


def json_to_wire(value: typing.Any) -> dict[str, typing.Any]:
    """
    Mark schemaless JSON so attachment-shaped objects stay opaque.
    """
    return {"__rspyts_json__": value}


def json_from_wire(value: object) -> typing.Any:
    """
    Unwrap schemaless JSON from its opaque wire wrapper.
    """
    if not isinstance(value, dict) or set(value) != {"__rspyts_json__"}:
        raise TypeError("expected an rspyts Json wrapper")
    wrapped = typing.cast(dict[str, typing.Any], value)
    return wrapped["__rspyts_json__"]


# Python int constrained to the signed 64-bit range and encoded as a string.
I64: typing.TypeAlias = typing.Annotated[
    int,
    pydantic.BeforeValidator(i64_host),
    pydantic.PlainSerializer(i64_to_wire, return_type=str),
]

# Python int constrained to the unsigned 64-bit range and encoded as a string.
U64: typing.TypeAlias = typing.Annotated[
    int,
    pydantic.BeforeValidator(u64_host),
    pydantic.PlainSerializer(u64_to_wire, return_type=str),
]

# Schemaless JSON encoded through an unambiguous wire wrapper.
JsonValue: typing.TypeAlias = typing.Annotated[
    typing.Any,
    pydantic.PlainSerializer(json_to_wire, return_type=dict[str, typing.Any]),
]
