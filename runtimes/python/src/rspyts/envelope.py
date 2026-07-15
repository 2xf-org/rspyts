"""Strict ABI 3 request and response envelope codecs."""

from __future__ import annotations

import collections.abc
import dataclasses
import json
import struct
import typing

import numpy as np

__all__ = ["DTYPES", "HEADER_LEN", "Response", "build_request", "decode_request", "parse_envelope"]

HEADER_LEN = 12
BUF_KEY = "__rspyts_buf__"

# Wire dtype names are frozen for ABI 3.
DTYPES: dict[str, type[np.generic]] = {
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

DTYPE_NAMES = {np.dtype(dtype): name for name, dtype in DTYPES.items()}
MAX_U32 = 2**32 - 1


@dataclasses.dataclass(frozen=True, slots=True)
class Response:
    """One decoded value together with its owned response attachment tail."""

    value: typing.Any
    tail: bytes = b""

    def __post_init__(self) -> None:
        if type(self.tail) is not bytes:
            raise TypeError(f"rspyts: response tail must be owned bytes, got {type(self.tail).__name__}")

    def child(self, value: typing.Any) -> Response:
        """Carry this response's attachment context to a schema-known child value."""
        return Response(value, self.tail)


def build_request(args_obj: collections.abc.Mapping[str, typing.Any] | None) -> bytes:
    """Encode structured arguments and nested attachments as an ABI 3 request."""
    tail = bytearray()

    def encode(value: typing.Any) -> typing.Any:
        if isinstance(value, (bytes, bytearray, memoryview)):
            data = bytes(value)
            off = len(tail)
            tail.extend(data)
            return {BUF_KEY: {"off": off, "len": len(data), "dt": "bytes"}}
        if isinstance(value, np.ndarray):
            native_dtype = value.dtype.newbyteorder("=")
            dt = DTYPE_NAMES.get(native_dtype)
            if dt is None:
                raise TypeError(f"rspyts: unsupported numpy buffer dtype {value.dtype}")
            dtype = np.dtype(DTYPES[dt])
            array = np.require(value, dtype=dtype, requirements=("C", "A"))
            align = dtype.itemsize
            tail.extend(b"\x00" * ((align - len(tail) % align) % align))
            off = len(tail)
            wire = array.astype(dtype.newbyteorder("<"), copy=False)
            tail.extend(wire.tobytes(order="C"))
            return {BUF_KEY: {"off": off, "len": array.size, "dt": dt}}
        if isinstance(value, collections.abc.Mapping):
            if any(type(key) is not str for key in value):
                raise TypeError("rspyts: request object keys must be exact strings")
            return {key: encode(item) for key, item in value.items()}
        if isinstance(value, (list, tuple)):
            return [encode(item) for item in value]
        if isinstance(value, float) and value == 0:
            return 0.0
        return value

    if args_obj is not None and not isinstance(args_obj, collections.abc.Mapping):
        raise TypeError("rspyts: request arguments must be a mapping or None")
    transformed = encode(args_obj or {})
    try:
        body = json.dumps(transformed, separators=(",", ":"), allow_nan=False).encode()
    except ValueError as exc:
        raise ValueError(
            "rspyts: non-finite floats (NaN/Infinity) cannot cross the bridge in "
            "JSON positions; pass them through slice or Buf parameters instead."
        ) from exc
    except (TypeError, OverflowError) as exc:
        raise TypeError(f"rspyts: request contains a non-JSON value: {exc}") from exc
    if len(body) > MAX_U32 or len(tail) > MAX_U32:
        raise ValueError("rspyts: request envelope component exceeds the 4 GiB limit")
    return bytes([0, 0, 0, 0]) + struct.pack("<II", len(body), len(tail)) + body + tail


def decode_request(raw: bytes) -> Response:
    """Strictly decode ABI 3 request framing without interpreting schema positions."""
    status, response = decode_frame(raw, request=True)
    assert status == 0
    return response


def parse_envelope(raw: bytes) -> tuple[int, Response]:
    """Decode one complete response without scanning schema-dependent object shapes."""
    status, response = decode_frame(raw, request=False)
    if status != 0 and response.tail:
        raise ValueError("rspyts: error and panic envelopes must not contain attachment bytes")
    return status, response


def decode_frame(raw: bytes, *, request: bool) -> tuple[int, Response]:
    """Validate and decode the shared envelope layout."""

    def reject_constant(token: str) -> typing.NoReturn:
        raise ValueError(f"non-standard JSON number {token}")

    if type(raw) is not bytes:
        raise TypeError(f"rspyts: envelope must be owned bytes, got {type(raw).__name__}")
    if len(raw) < HEADER_LEN:
        raise ValueError(f"rspyts: truncated envelope header: expected {HEADER_LEN} bytes, got {len(raw)}")
    status = raw[0]
    if request and status != 0:
        raise ValueError(f"rspyts: invalid request marker {status}; expected 0")
    if not request and status not in (0, 1, 2):
        raise ValueError(f"rspyts: invalid response status {status}; expected 0, 1, or 2")
    if raw[1:4] != b"\x00\x00\x00":
        raise ValueError("rspyts: reserved envelope header bytes must be zero")
    json_len, tail_len = struct.unpack_from("<II", raw, 4)
    total = HEADER_LEN + json_len + tail_len
    if len(raw) < total:
        raise ValueError(f"rspyts: truncated envelope: header declares {total} bytes, got {len(raw)}")
    if len(raw) > total:
        raise ValueError(f"rspyts: trailing bytes after envelope: header declares {total} bytes, got {len(raw)}")
    json_end = HEADER_LEN + json_len
    try:
        payload = json.loads(
            raw[HEADER_LEN:json_end],
            parse_constant=reject_constant,
        )
    except (UnicodeDecodeError, json.JSONDecodeError, ValueError) as exc:
        raise ValueError(f"rspyts: malformed envelope JSON: {exc}") from exc
    return status, Response(payload, raw[json_end:total])


def materialize_attachment(response: Response, expected_dtype: str) -> bytes | np.ndarray:
    """Validate and copy one attachment at a schema-declared response position."""
    obj = response.value
    if type(obj) is not dict or set(obj) != {BUF_KEY}:
        raise TypeError("rspyts: expected a buffer attachment wrapper")
    spec = obj[BUF_KEY]
    if type(spec) is not dict or set(spec) != {"off", "len", "dt"}:
        raise ValueError("rspyts: malformed buffer placeholder body")
    off, length, dt = spec["off"], spec["len"], spec["dt"]
    if (
        type(off) is not int
        or type(length) is not int
        or type(dt) is not str
        or off < 0
        or length < 0
        or (dt != "bytes" and dt not in DTYPES)
    ):
        raise ValueError(f"rspyts: malformed buffer placeholder: {spec!r}")
    if dt != expected_dtype:
        raise TypeError(f"rspyts: expected a {expected_dtype} buffer attachment, got {dt}")
    if dt == "bytes":
        end = off + length
        if end > len(response.tail):
            raise ValueError(f"rspyts: buffer range {off}..{end} exceeds tail length {len(response.tail)}")
        return bytes(response.tail[off:end])
    dtype = np.dtype(DTYPES[dt])
    if off % dtype.itemsize != 0:
        raise ValueError(f"rspyts: buffer offset {off} is not aligned to {dtype.itemsize} bytes for {dt}")
    end = off + length * dtype.itemsize
    if end > len(response.tail):
        raise ValueError(f"rspyts: buffer range {off}..{end} exceeds tail length {len(response.tail)}")
    wire_dtype = dtype.newbyteorder("<")
    array = np.frombuffer(response.tail, dtype=wire_dtype, count=length, offset=off)
    return array.astype(dtype, copy=True)
