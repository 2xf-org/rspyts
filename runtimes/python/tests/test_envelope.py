"""
Envelope decoding tests (ABI §4, §6) — pure bytes, no cdylib.
"""

import json
import struct
from typing import Any

import numpy as np
import pytest

from rspyts import envelope


def make_envelope(status: int, payload: Any, tail: bytes = b"") -> bytes:
    """
    Assemble an envelope exactly as the Rust side does (envelope.rs seal).

    Args:
        status: The status byte.
        payload: The JSON payload.
        tail: The raw numeric tail.

    Returns:
        The complete envelope bytes.
    """
    body = json.dumps(payload, separators=(",", ":")).encode()
    header = bytes([status, 0, 0, 0]) + struct.pack("<II", len(body), len(tail))
    assert len(header) == envelope.HEADER_LEN
    return header + body + tail


def placeholder(off: int, length: int, dt: str) -> dict[str, Any]:
    return {"__rspyts_buf__": {"off": off, "len": length, "dt": dt}}


def test_ok_scalar_payload():
    assert envelope.parse_envelope(make_envelope(0, 41)) == (0, 41)


def test_ok_object_payload():
    status, payload = envelope.parse_envelope(make_envelope(0, {"a": 1, "b": [True, None, "x"]}))
    assert status == 0
    assert payload == {"a": 1, "b": [True, None, "x"]}


def test_null_payload_for_unit_returns():
    assert envelope.parse_envelope(make_envelope(0, None)) == (0, None)


@pytest.mark.parametrize("status", [1, 2])
def test_error_statuses_pass_through(status):
    got_status, payload = envelope.parse_envelope(make_envelope(status, {"code": "x", "message": "y"}))
    assert got_status == status
    assert payload == {"code": "x", "message": "y"}


def test_buffer_substitution_with_offset_and_alignment():
    # Tail layout mirrors Rust tail_push: a u8 at offset 0, then zero
    # padding so the f64 data starts at its natural alignment (offset 8).
    values = np.array([1.5, -2.25, 3.0], dtype=np.float64)
    tail = b"\x07" + b"\x00" * 7 + values.tobytes()
    payload = {"raw": placeholder(0, 1, "u8"), "vals": placeholder(8, 3, "f64")}

    status, decoded = envelope.parse_envelope(make_envelope(0, payload, tail))

    assert status == 0
    assert decoded["raw"].dtype == np.uint8
    assert decoded["raw"].tolist() == [7]
    assert decoded["vals"].dtype == np.float64
    np.testing.assert_array_equal(decoded["vals"], values)


@pytest.mark.parametrize(
    ("dt", "np_dtype"),
    [
        ("u8", np.uint8),
        ("i16", np.int16),
        ("i32", np.int32),
        ("f32", np.float32),
        ("f64", np.float64),
    ],
)
def test_every_wire_dtype_decodes(dt, np_dtype):
    values = np.array([1, 2, 3], dtype=np_dtype)
    status, decoded = envelope.parse_envelope(make_envelope(0, placeholder(0, 3, dt), values.tobytes()))
    assert status == 0
    assert decoded.dtype == np_dtype
    np.testing.assert_array_equal(decoded, values)


def test_substitution_nested_in_lists_and_dicts():
    inner = np.array([9, 8], dtype=np.int32)
    obj = {
        "reports": [
            {"name": "a", "samples": placeholder(0, 2, "i32")},
            {"name": "b", "samples": None},
        ],
        "by_key": {"x": [placeholder(0, 2, "i32")]},
    }
    decoded = envelope.substitute_buffers(obj, inner.tobytes())
    np.testing.assert_array_equal(decoded["reports"][0]["samples"], inner)
    assert decoded["reports"][1]["samples"] is None
    np.testing.assert_array_equal(decoded["by_key"]["x"][0], inner)


def test_empty_buffer_decodes_to_empty_array():
    decoded = envelope.substitute_buffers(placeholder(0, 0, "f32"), b"")
    assert decoded.dtype == np.float32
    assert decoded.size == 0


def test_decoded_arrays_are_owned_copies():
    values = np.array([1.0], dtype=np.float64)
    decoded = envelope.substitute_buffers(placeholder(0, 1, "f64"), values.tobytes())
    # np.frombuffer over bytes is read-only; the contract is a mutable copy
    # that no longer references envelope memory.
    assert decoded.flags.writeable
    assert decoded.flags.owndata


def test_only_exact_single_key_dicts_are_substituted():
    obj = {"__rspyts_buf__": {"off": 0, "len": 1, "dt": "u8"}, "other": 1}
    decoded = envelope.substitute_buffers(obj, b"\x01")
    # Two keys: not a placeholder, walked as a plain dict.
    assert decoded == obj


def test_non_ascii_json_payload():
    status, payload = envelope.parse_envelope(make_envelope(0, {"msg": "café ☕"}))
    assert status == 0
    assert payload == {"msg": "café ☕"}


def test_placeholder_nested_dict_in_list_in_dict_three_deep():
    values = np.array([4, 5, 6], dtype=np.int16)
    obj = {"outer": [{"middle": {"inner": placeholder(0, 3, "i16")}}]}
    decoded = envelope.substitute_buffers(obj, values.tobytes())
    np.testing.assert_array_equal(decoded["outer"][0]["middle"]["inner"], values)


def test_multiple_placeholders_share_one_tail_with_padding():
    # One tail laid out as Rust tail_push would: each buffer starts at its
    # dtype's natural alignment, with zero padding in the gaps.
    bytes_u8 = np.array([1, 2, 3], dtype=np.uint8)
    counts = np.array([-100], dtype=np.int32)
    means = np.array([2.5, -0.5], dtype=np.float64)
    tail = (
        bytes_u8.tobytes()
        + b"\x00"  # pad 3 -> 4 for the i32
        + counts.tobytes()
        + means.tobytes()  # 8 is already f64-aligned
    )
    payload = {
        "raw": placeholder(0, 3, "u8"),
        "counts": placeholder(4, 1, "i32"),
        "means": placeholder(8, 2, "f64"),
    }

    status, decoded = envelope.parse_envelope(make_envelope(0, payload, tail))

    assert status == 0
    np.testing.assert_array_equal(decoded["raw"], bytes_u8)
    np.testing.assert_array_equal(decoded["counts"], counts)
    np.testing.assert_array_equal(decoded["means"], means)


def test_buffer_slice_ends_exactly_at_tail_boundary():
    values = np.array([10, 20], dtype=np.float32)
    tail = b"\x00" * 4 + values.tobytes()
    decoded = envelope.substitute_buffers(placeholder(4, 2, "f32"), tail)
    # off + len * itemsize == len(tail): the last byte is included, exactly.
    np.testing.assert_array_equal(decoded, values)


def test_zero_len_buffer_at_tail_end():
    decoded = envelope.substitute_buffers(placeholder(3, 0, "u8"), b"abc")
    assert decoded.size == 0


def test_bytes_beyond_declared_tail_len_are_ignored():
    values = np.array([9], dtype=np.uint8)
    raw = make_envelope(0, placeholder(0, 1, "u8"), values.tobytes()) + b"trailing junk"
    status, decoded = envelope.parse_envelope(raw)
    assert status == 0
    np.testing.assert_array_equal(decoded, values)


@pytest.mark.parametrize("length", [1, 4, 8, 11])
def test_short_header_raises_clear_struct_error(length):
    with pytest.raises(struct.error, match="at least"):
        envelope.parse_envelope(bytes(length))


def test_empty_envelope_raises():
    with pytest.raises(IndexError):
        envelope.parse_envelope(b"")


def test_status_json_tail_extraction_over_size_grid():
    # Property-style sweep with hand-rolled cases: every combination of
    # status byte, JSON payload size, and tail size must round-trip.
    sizes = [0, 1, 7, 12, 64, 255, 256, 1024]
    for status in (0, 1, 2):
        for pad in sizes:
            for tail_len in sizes:
                tail = bytes((i * 31 + 7) % 256 for i in range(tail_len))
                payload = {"pad": "x" * pad, "buf": placeholder(0, tail_len, "u8")}

                got_status, decoded = envelope.parse_envelope(make_envelope(status, payload, tail))

                assert got_status == status
                assert decoded["pad"] == "x" * pad
                assert decoded["buf"].tobytes() == tail
