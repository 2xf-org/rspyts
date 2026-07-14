"""
Strict request and response envelope codecs.
"""

from __future__ import annotations

import collections.abc
import json
import struct
import typing

import numpy as np

__all__ = [
    "DTYPES",
    "HEADER_LEN",
    "build_request",
    "decode_request",
    "parse_envelope",
    "substitute_buffers",
]

HEADER_LEN = 12
BUF_KEY = "__rspyts_buf__"
JSON_KEY = "__rspyts_json__"

# Wire dtype names (ABI §6) to numpy dtypes. Also used by Library.call for
# input slices; the wire names are frozen — never extend without an ABI bump.
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


def build_request(args_obj: collections.abc.Mapping[str, typing.Any] | None) -> bytes:
    """
    Encode arguments and nested numpy attachments as an ABI-2 request.
    """
    tail = bytearray()

    def encode(value: typing.Any) -> typing.Any:
        if isinstance(value, collections.abc.Mapping) and set(value) == {JSON_KEY}:
            # Schemaless Json is opaque to attachment discovery. json.dumps
            # below still rejects non-JSON Python objects and non-finite
            # numbers inside the wrapper.
            return {JSON_KEY: value[JSON_KEY]}
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
            return {key: encode(item) for key, item in value.items()}
        if isinstance(value, (list, tuple)):
            return [encode(item) for item in value]
        if isinstance(value, float) and value == 0:
            return 0.0  # canonicalize the JSON representation of -0.0
        return value

    transformed = encode(args_obj or {})
    try:
        body = json.dumps(transformed, separators=(",", ":"), allow_nan=False).encode()
    except ValueError as exc:
        raise ValueError(
            "rspyts: non-finite floats (NaN/Infinity) cannot cross the bridge in "
            "JSON positions; pass them through slice or Buf parameters instead."
        ) from exc
    if len(body) > MAX_U32 or len(tail) > MAX_U32:
        raise ValueError("rspyts: request envelope component exceeds the 4 GiB limit")
    return bytes([0, 0, 0, 0]) + struct.pack("<II", len(body), len(tail)) + body + tail


def decode_request(raw: bytes) -> tuple[typing.Any, bytes]:
    """
    Strictly decode an ABI-2 request for diagnostics.
    """
    status, payload, tail = decode_frame(raw, request=True)
    assert status == 0
    walk_buffers(payload, tail, materialize=False)
    return payload, tail


def parse_envelope(raw: bytes) -> tuple[int, typing.Any]:
    """
    Decode a complete envelope into ``(status, payload)``.

    Args:
        raw: The full allocation (header + JSON + tail).

    Returns:
        The status byte and the payload, with every buffer placeholder
        replaced per :func:`substitute_buffers`.
    """
    status, payload, tail = decode_frame(raw, request=False)
    if status == 0:
        return status, walk_buffers(payload, tail, materialize=True)
    if tail:
        raise ValueError("rspyts: error and panic envelopes must not contain attachment bytes")
    return status, payload


def decode_frame(raw: bytes, *, request: bool) -> tuple[int, typing.Any, bytes]:
    """
    Validate and decode the shared envelope layout.

    Args:
        raw: Complete envelope bytes.
        request: Whether to apply request marker rules.

    Returns:
        Status, decoded JSON payload, and binary tail.
    """
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
            parse_constant=lambda token: (_ for _ in ()).throw(ValueError(f"non-standard JSON number {token}")),
        )
    except (UnicodeDecodeError, json.JSONDecodeError, ValueError) as exc:
        raise ValueError(f"rspyts: malformed envelope JSON: {exc}") from exc
    return status, payload, raw[json_end:total]


def substitute_buffers(obj: typing.Any, tail: bytes) -> typing.Any:
    """
    Recursively replace ``__rspyts_buf__`` placeholders with numpy arrays.

    Notes:
        A dict that is exactly ``{"__rspyts_buf__": {...}}`` becomes an
        array copied out of ``tail``; dicts and lists are walked at any
        depth; all other values pass through unchanged. The single-key
        check cannot misfire on user data: wire field names come from Rust
        identifiers and can never be ``__rspyts_buf__``.

    Args:
        obj: The decoded JSON payload (or any fragment of it).
        tail: The raw numeric tail of the envelope.

    Returns:
        The payload with every placeholder replaced by an owned array.
    """
    return walk_buffers(obj, tail, materialize=True)


def walk_buffers(obj: typing.Any, tail: bytes, *, materialize: bool) -> typing.Any:
    """
    Validate placeholders and optionally copy their attachment values.

    Args:
        obj: JSON value to walk.
        tail: Binary attachment tail.
        materialize: Whether to replace placeholders with host values.

    Returns:
        The walked value.
    """
    if isinstance(obj, dict):
        if set(obj) == {JSON_KEY}:
            return obj[JSON_KEY] if materialize else obj
        if BUF_KEY in obj:
            if len(obj) != 1:
                raise ValueError("rspyts: buffer placeholder cannot have sibling fields")
            spec = obj[BUF_KEY]
            if not isinstance(spec, dict) or set(spec) != {"off", "len", "dt"}:
                raise ValueError("rspyts: malformed buffer placeholder body")
            off, length, dt = spec["off"], spec["len"], spec["dt"]
            if (
                type(off) is not int
                or type(length) is not int
                or not isinstance(dt, str)
                or off < 0
                or length < 0
                or (dt != "bytes" and dt not in DTYPES)
            ):
                raise ValueError(f"rspyts: malformed buffer placeholder: {spec!r}")
            if dt == "bytes":
                end = off + length
                if end > len(tail):
                    raise ValueError(f"rspyts: buffer range {off}..{end} exceeds tail length {len(tail)}")
                return bytes(tail[off:end]) if materialize else obj
            dtype = np.dtype(DTYPES[dt])
            if off % dtype.itemsize != 0:
                raise ValueError(f"rspyts: buffer offset {off} is not aligned to {dtype.itemsize} bytes for {dt}")
            end = off + length * dtype.itemsize
            if end > len(tail):
                raise ValueError(f"rspyts: buffer range {off}..{end} exceeds tail length {len(tail)}")
            if not materialize:
                return obj
            wire_dtype = dtype.newbyteorder("<")
            array = np.frombuffer(tail, dtype=wire_dtype, count=length, offset=off)
            return array.astype(dtype, copy=True)
        return {key: walk_buffers(value, tail, materialize=materialize) for key, value in obj.items()}
    if isinstance(obj, list):
        return [walk_buffers(item, tail, materialize=materialize) for item in obj]
    return obj
