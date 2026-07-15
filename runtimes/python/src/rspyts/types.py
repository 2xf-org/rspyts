"""Internal ABI 3 schema codecs used through :mod:`rspyts._internal`."""

from __future__ import annotations

import math
import typing

import numpy as np

from . import envelope

I64_MIN = -(2**63)
I64_MAX = 2**63 - 1
U64_MAX = 2**64 - 1
JSON_SAFE_INTEGER_MAX = 2**53 - 1
F32_MAX = float(np.finfo(np.float32).max)

_BUFFER_DTYPES: dict[str, type[np.generic]] = {
    "u8": np.uint8,
    "i8": np.int8,
    "u16": np.uint16,
    "i16": np.int16,
    "u32": np.uint32,
    "i32": np.int32,
    "u64": np.uint64,
    "i64": np.int64,
    "f32": np.float32,
    "f64": np.float64,
}


def _response(value: object) -> envelope.Response:
    if type(value) is not envelope.Response:
        raise TypeError(f"rspyts: wire decoder requires a Response, got {type(value).__name__}")
    return value


def bool_from_wire(response: envelope.Response) -> bool:
    """Validate an exact JSON boolean."""
    value = _response(response).value
    if type(value) is not bool:
        raise TypeError(f"rspyts: expected a JSON boolean, got {type(value).__name__}")
    return value


def bounded_int_from_wire(response: envelope.Response, *, minimum: int, maximum: int) -> int:
    """Validate an exact, bounded JSON integer."""
    value = _response(response).value
    if type(value) is not int:
        raise TypeError(f"rspyts: expected a JSON integer, got {type(value).__name__}")
    if not minimum <= value <= maximum:
        raise ValueError(f"rspyts: integer value out of range {minimum}..={maximum}: {value}")
    return value


def string_from_wire(response: envelope.Response) -> str:
    """Validate an exact JSON string."""
    value = _response(response).value
    if type(value) is not str:
        raise TypeError(f"rspyts: expected a JSON string, got {type(value).__name__}")
    return value


def bytes_from_wire(response: envelope.Response) -> bytes:
    """Materialize one schema-declared opaque bytes attachment."""
    return typing.cast(bytes, envelope.materialize_attachment(_response(response), "bytes"))


def list_from_wire(response: envelope.Response) -> list[envelope.Response]:
    """Validate a JSON array and preserve tail context for every child."""
    response = _response(response)
    if type(response.value) is not list:
        raise TypeError(f"rspyts: expected a JSON array, got {type(response.value).__name__}")
    return [response.child(item) for item in response.value]


def map_from_wire(response: envelope.Response) -> dict[str, envelope.Response]:
    """Validate a JSON object and preserve tail context for every child."""
    response = _response(response)
    if type(response.value) is not dict:
        raise TypeError(f"rspyts: expected a JSON object, got {type(response.value).__name__}")
    if any(type(key) is not str for key in response.value):
        raise TypeError("rspyts: expected every JSON object key to be a string")
    return {key: response.child(value) for key, value in response.value.items()}


def null_from_wire(response: envelope.Response) -> None:
    """Validate the exact JSON null value."""
    value = _response(response).value
    if value is not None:
        raise TypeError(f"rspyts: expected JSON null, got {type(value).__name__}")
    return None


def tuple_from_wire(response: envelope.Response, *, length: int) -> list[envelope.Response]:
    """Validate the JSON-array representation of a fixed-length tuple."""
    items = list_from_wire(response)
    if len(items) != length:
        raise ValueError(f"rspyts: expected a JSON array of length {length}, got {len(items)}")
    return items


def buffer_from_wire(response: envelope.Response, *, dtype: str) -> np.ndarray:
    """Materialize one schema-declared numeric-buffer attachment."""
    expected = _BUFFER_DTYPES.get(dtype)
    if expected is None:
        raise ValueError(f"rspyts: unsupported buffer dtype {dtype!r}")
    value = envelope.materialize_attachment(_response(response), dtype)
    if type(value) is not np.ndarray:
        raise TypeError(f"rspyts: expected a numpy {dtype} buffer attachment")
    expected_dtype = np.dtype(expected)
    if value.ndim != 1 or value.dtype != expected_dtype:
        raise TypeError(
            f"rspyts: expected a one-dimensional numpy {dtype} buffer, got shape={value.shape} dtype={value.dtype}"
        )
    return value


def enum_from_wire(
    response: envelope.Response,
    *,
    tag: str,
    variants: typing.Mapping[str, typing.Callable[[envelope.Response], typing.Any]],
) -> typing.Any:
    """Validate and dispatch an internally tagged data-enum object."""
    obj = map_from_wire(response)
    if tag not in obj:
        raise TypeError(f"rspyts: expected required enum tag {tag!r}")
    discriminator = string_from_wire(obj[tag])
    try:
        decoder = variants[discriminator]
    except KeyError as error:
        raise ValueError(f"rspyts: unknown {tag!r} discriminator {discriminator!r}") from error
    return decoder(response)


def _exact_int(value: object, *, signed: bool, allow_host_int: bool) -> int:
    """Validate one exact host integer or canonical wire decimal string."""
    label = "signed" if signed else "unsigned"
    if allow_host_int:
        if type(value) is not int:
            raise TypeError(f"expected a {label} 64-bit integer, got {type(value).__name__}")
        parsed = value
    elif type(value) is str:
        try:
            parsed = int(value, 10)
        except ValueError as error:
            raise ValueError(f"expected a canonical {label} 64-bit decimal string") from error
        if str(parsed) != value:
            raise ValueError(f"expected a canonical {label} 64-bit decimal string")
    else:
        raise TypeError(f"expected a {label} 64-bit integer as a canonical decimal string, got {type(value).__name__}")

    minimum = I64_MIN if signed else 0
    maximum = I64_MAX if signed else U64_MAX
    if not minimum <= parsed <= maximum:
        kind = "i64" if signed else "u64"
        raise ValueError(f"{kind} value out of range: {parsed}")
    return parsed


def i64_from_wire(response: envelope.Response) -> int:
    """Validate a canonical signed 64-bit wire string."""
    return _exact_int(_response(response).value, signed=True, allow_host_int=False)


def u64_from_wire(response: envelope.Response) -> int:
    """Validate a canonical unsigned 64-bit wire string."""
    return _exact_int(_response(response).value, signed=False, allow_host_int=False)


def i64_to_wire(value: object) -> str:
    """Encode a Python integer as a canonical signed wire string."""
    return str(_exact_int(value, signed=True, allow_host_int=True))


def u64_to_wire(value: object) -> str:
    """Encode a Python integer as a canonical unsigned wire string."""
    return str(_exact_int(value, signed=False, allow_host_int=True))


def float_from_wire(response: envelope.Response, *, f32: bool = False) -> float:
    """Validate a finite structured float and canonicalize signed zero."""
    value = _response(response).value
    if type(value) not in (int, float):
        raise TypeError(f"rspyts: expected a finite JSON number, got {type(value).__name__}")
    numeric = typing.cast(int | float, value)
    if f32 and abs(numeric) > F32_MAX:
        raise ValueError("rspyts: expected a finite f32 JSON number")
    parsed = float(numeric)
    if not math.isfinite(parsed):
        raise ValueError("rspyts: expected a finite JSON number")
    return 0.0 if parsed == 0 else parsed


def _checked_json_value(value: typing.Any, active: set[int]) -> typing.Any:
    """Validate and copy one portable JSON value, canonicalizing signed zero."""
    value_type = type(value)
    if value is None or value_type in (bool, str):
        return value
    if value_type is int:
        if abs(value) > JSON_SAFE_INTEGER_MAX:
            raise ValueError("schemaless JSON integral numbers must be safe integers")
        return value
    if value_type is float:
        if not math.isfinite(value):
            raise ValueError("schemaless JSON numbers must be finite")
        if value.is_integer() and abs(value) > JSON_SAFE_INTEGER_MAX:
            raise ValueError("schemaless JSON integral numbers must be safe integers")
        return 0.0 if value == 0 else value
    if value_type not in (list, dict):
        raise TypeError(
            "schemaless JSON values must use only null, exact booleans, numbers, "
            f"strings, lists, and string-keyed dictionaries; got {value_type.__name__}"
        )

    identity = id(value)
    if identity in active:
        raise ValueError("schemaless JSON values cannot contain reference cycles")
    active.add(identity)
    try:
        if value_type is list:
            return [_checked_json_value(item, active) for item in value]
        for key in value:
            if type(key) is not str:
                raise TypeError("schemaless JSON object keys must be exact strings")
        return {key: _checked_json_value(item, active) for key, item in value.items()}
    finally:
        active.remove(identity)


def json_to_wire(value: typing.Any) -> typing.Any:
    """Validate a transparent schemaless JSON argument without adding a wire wrapper."""
    return _checked_json_value(value, set())


def json_from_wire(response: envelope.Response) -> typing.Any:
    """Return a schema-declared JSON value without traversing reserved-looking objects."""
    return _checked_json_value(_response(response).value, set())
