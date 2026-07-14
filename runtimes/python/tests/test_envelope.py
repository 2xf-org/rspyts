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


@pytest.mark.parametrize("status", [1, 2])
def test_error_statuses_reject_attachment_tails(status):
    with pytest.raises(ValueError, match="must not contain attachment bytes"):
        envelope.parse_envelope(make_envelope(status, {"code": "x", "message": "y"}, b"x"))


def test_json_wrapper_keeps_marker_shaped_content_opaque_and_unwraps_it():
    marker = placeholder(0, 1, "u8")
    wrapped = {"__rspyts_json__": {"marker": marker, "nested": [{"marker": marker}]}}
    assert envelope.parse_envelope(make_envelope(0, wrapped)) == (0, wrapped["__rspyts_json__"])

    payload, tail = envelope.decode_request(envelope.build_request({"value": wrapped}))
    assert payload == {"value": wrapped}
    assert tail == b""


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
    ("dt", "np_dtype", "items"),
    [
        ("u8", np.uint8, [0, 255]),
        ("i8", np.int8, [-128, 127]),
        ("u16", np.uint16, [0, 65535]),
        ("i16", np.int16, [-32768, 32767]),
        ("u32", np.uint32, [0, 2**32 - 1]),
        ("i32", np.int32, [-(2**31), 2**31 - 1]),
        ("u64", np.uint64, [0, 2**64 - 1]),
        ("i64", np.int64, [-(2**63), 2**63 - 1]),
        ("f32", np.float32, [-0.5, 1024.25]),
        ("f64", np.float64, [-1e300, 0.1]),
    ],
)
def test_every_wire_dtype_decodes(dt, np_dtype, items):
    values = np.array(items, dtype=np_dtype)
    status, decoded = envelope.parse_envelope(make_envelope(0, placeholder(0, len(items), dt), values.tobytes()))
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


def test_placeholder_key_with_siblings_is_rejected():
    obj = {"__rspyts_buf__": {"off": 0, "len": 1, "dt": "u8"}, "other": 1}
    with pytest.raises(ValueError, match="sibling fields"):
        envelope.substitute_buffers(obj, b"\x01")


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


def test_bytes_beyond_declared_tail_len_are_rejected():
    values = np.array([9], dtype=np.uint8)
    raw = make_envelope(0, placeholder(0, 1, "u8"), values.tobytes()) + b"trailing junk"
    with pytest.raises(ValueError, match="trailing bytes"):
        envelope.parse_envelope(raw)


@pytest.mark.parametrize("length", [0, 1, 4, 8, 11])
def test_short_header_raises_clear_protocol_error(length):
    with pytest.raises(ValueError, match="truncated envelope header"):
        envelope.parse_envelope(bytes(length))


def test_status_json_tail_extraction_over_size_grid():
    # Property-style sweep with hand-rolled cases: every combination of
    # status byte, JSON payload size, and tail size must round-trip.
    sizes = [0, 1, 7, 12, 64, 255, 256, 1024]
    for pad in sizes:
        for tail_len in sizes:
            tail = bytes((i * 31 + 7) % 256 for i in range(tail_len))
            payload = {"pad": "x" * pad, "buf": placeholder(0, tail_len, "u8")}

            got_status, decoded = envelope.parse_envelope(make_envelope(0, payload, tail))

            assert got_status == 0
            assert decoded["pad"] == "x" * pad
            assert decoded["buf"].tobytes() == tail


def test_build_request_encodes_nested_aligned_little_endian_attachments():
    arrays = {
        "u8": np.array([0, 255], dtype=np.uint8),
        "i8": np.array([-128, 127], dtype=np.int8),
        "u16": np.array([0, 65535], dtype=np.uint16),
        "i16": np.array([-32768, 32767], dtype=np.int16),
        "u32": np.array([0, 2**32 - 1], dtype=np.uint32),
        "i32": np.array([-(2**31), 2**31 - 1], dtype=np.int32),
        "u64": np.array([0, 2**64 - 1], dtype=np.uint64),
        "i64": np.array([-(2**63), 2**63 - 1], dtype=np.int64),
        "f32": np.array([-0.5, 1.25], dtype=np.float32),
        "f64": np.array([-1e300, 0.1], dtype=np.float64),
    }
    request = envelope.build_request({"nested": [{name: value} for name, value in arrays.items()]})
    payload, tail = envelope.decode_request(request)

    for index, (name, array) in enumerate(arrays.items()):
        spec = payload["nested"][index][name]["__rspyts_buf__"]
        assert spec["dt"] == name
        assert spec["len"] == array.size
        assert spec["off"] % array.dtype.itemsize == 0
        expected = array.astype(array.dtype.newbyteorder("<"), copy=False).tobytes()
        assert tail[spec["off"] : spec["off"] + len(expected)] == expected


def test_build_request_plain_values_have_empty_tail_and_exact_header():
    request = envelope.build_request({"a": 1, "items": [True, None, "x"]})
    payload, tail = envelope.decode_request(request)
    assert payload == {"a": 1, "items": [True, None, "x"]}
    assert tail == b""
    assert request[:4] == b"\x00\x00\x00\x00"
    assert len(request) == envelope.HEADER_LEN + struct.unpack_from("<I", request, 4)[0]


def test_bytes_and_numeric_u8_use_distinct_attachment_dtypes():
    request = envelope.build_request(
        {
            "bytes": bytes([0x00, 0xFF]),
            "mutable": bytearray([1, 2]),
            "numeric": np.array([3, 4], dtype=np.uint8),
        }
    )
    payload, tail = envelope.decode_request(request)
    assert payload["bytes"]["__rspyts_buf__"]["dt"] == "bytes"
    assert payload["mutable"]["__rspyts_buf__"]["dt"] == "bytes"
    assert payload["numeric"]["__rspyts_buf__"]["dt"] == "u8"

    decoded = envelope.substitute_buffers(payload, tail)
    assert decoded["bytes"] == b"\x00\xff"
    assert decoded["mutable"] == b"\x01\x02"
    assert isinstance(decoded["numeric"], np.ndarray)
    assert decoded["numeric"].dtype == np.uint8


def test_bytes_response_is_an_owned_bytes_value():
    status, value = envelope.parse_envelope(make_envelope(0, placeholder(0, 3, "bytes"), b"\x00\x7f\xff"))
    assert status == 0
    assert value == b"\x00\x7f\xff"


def test_build_request_rejects_unsupported_arrays_and_non_finite_json():
    with pytest.raises(TypeError, match="unsupported numpy buffer dtype"):
        envelope.build_request({"values": np.array([1 + 2j], dtype=np.complex128)})
    with pytest.raises(ValueError, match="non-finite floats"):
        envelope.build_request({"value": float("nan")})


def test_direction_specific_status_and_reserved_bytes_are_strict():
    request = bytearray(envelope.build_request({}))
    request[0] = 1
    with pytest.raises(ValueError, match="invalid request marker"):
        envelope.decode_request(bytes(request))

    response = bytearray(make_envelope(0, None))
    response[0] = 3
    with pytest.raises(ValueError, match="invalid response status"):
        envelope.parse_envelope(bytes(response))

    response = bytearray(make_envelope(0, None))
    response[2] = 1
    with pytest.raises(ValueError, match="reserved envelope"):
        envelope.parse_envelope(bytes(response))


def test_declared_length_truncation_and_malformed_json_are_rejected():
    response = make_envelope(0, {"a": 1})
    with pytest.raises(ValueError, match="truncated envelope"):
        envelope.parse_envelope(response[:-1])

    malformed = bytes([0, 0, 0, 0]) + struct.pack("<II", 1, 0) + b"{"
    with pytest.raises(ValueError, match="malformed envelope JSON"):
        envelope.parse_envelope(malformed)

    nonstandard = bytes([0, 0, 0, 0]) + struct.pack("<II", 3, 0) + b"NaN"
    with pytest.raises(ValueError, match="non-standard JSON number"):
        envelope.parse_envelope(nonstandard)


@pytest.mark.parametrize(
    ("body", "match"),
    [
        (None, "malformed buffer placeholder body"),
        ({"off": 0, "len": 1}, "malformed buffer placeholder body"),
        ({"off": -1, "len": 1, "dt": "u8"}, "malformed buffer placeholder"),
        ({"off": 1, "len": 1, "dt": "i16"}, "not aligned"),
        ({"off": 0, "len": 2, "dt": "u8"}, "exceeds tail length"),
    ],
)
def test_malformed_buffer_placeholders_are_rejected(body, match):
    raw = make_envelope(0, {"__rspyts_buf__": body}, b"\x00")
    with pytest.raises(ValueError, match=match):
        envelope.parse_envelope(raw)
