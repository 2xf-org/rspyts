"""
Decoding of the response envelope (ABI §4) and its raw tail (ABI §6).

Notes:
    Every bridged call returns one allocation::

        offset  size       field
        0       1          status   (0 ok, 1 error, 2 panic)
        1       3          reserved (zero)
        4       4          json_len (u32 LE)
        8       4          tail_len (u32 LE)
        12      json_len   UTF-8 JSON payload
        12+j    tail_len   raw numeric tail (Buf<T> data)

    ``Buf<T>`` values inside the JSON appear as placeholder objects
    ``{"__rspyts_buf__": {"off": …, "len": …, "dt": …}}`` pointing into the
    tail; decoding replaces each with a ``numpy.ndarray`` copied out of the
    envelope, so the returned payload never aliases envelope memory.

    Everything here is a pure function of ``bytes`` — no cdylib required —
    which is what makes the format unit-testable in isolation.
"""

from __future__ import annotations

import json
import struct
from typing import Any

import numpy as np

__all__ = ["DTYPES", "HEADER_LEN", "parse_envelope", "substitute_buffers"]

HEADER_LEN = 12
BUF_KEY = "__rspyts_buf__"

# Wire dtype names (ABI §6) to numpy dtypes. Also used by Library.call for
# input slices; the wire names are frozen — never extend without an ABI bump.
DTYPES: dict[str, type[np.generic]] = {
    "u8": np.uint8,
    "i16": np.int16,
    "i32": np.int32,
    "f32": np.float32,
    "f64": np.float64,
}


def parse_envelope(raw: bytes) -> tuple[int, Any]:
    """
    Decode a complete envelope into ``(status, payload)``.

    Args:
        raw: The full allocation (header + JSON + tail).

    Returns:
        The status byte and the payload, with every buffer placeholder
        replaced per :func:`substitute_buffers`.
    """
    status = raw[0]
    json_len, tail_len = struct.unpack_from("<II", raw, 4)
    json_end = HEADER_LEN + json_len
    obj = json.loads(raw[HEADER_LEN:json_end])
    tail = raw[json_end : json_end + tail_len]
    return status, substitute_buffers(obj, tail)


def substitute_buffers(obj: Any, tail: bytes) -> Any:
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
    if isinstance(obj, dict):
        if len(obj) == 1 and BUF_KEY in obj:
            spec = obj[BUF_KEY]
            arr = np.frombuffer(
                tail,
                dtype=DTYPES[spec["dt"]],
                count=spec["len"],
                offset=spec["off"],
            )
            return arr.copy()
        return {key: substitute_buffers(value, tail) for key, value in obj.items()}
    if isinstance(obj, list):
        return [substitute_buffers(item, tail) for item in obj]
    return obj
