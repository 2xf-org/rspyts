"""Pure-byte ABI 3 envelope and attachment tests."""

import json
import struct
from typing import Any

import numpy as np
import pytest

from rspyts import _internal, envelope


def make_envelope(status: int, payload: Any, tail: bytes = b"") -> bytes:
    body = json.dumps(payload, separators=(",", ":")).encode()
    return bytes([status, 0, 0, 0]) + struct.pack("<II", len(body), len(tail)) + body + tail


def placeholder(off: int, length: int, dt: str) -> dict[str, Any]:
    return {"__rspyts_buf__": {"off": off, "len": length, "dt": dt}}


def test_response_is_explicit_owned_value_and_tail_context():
    raw = make_envelope(0, {"value": [1, 2]}, b"tail")
    status, response = envelope.parse_envelope(raw)
    assert status == 0
    assert type(response) is _internal.Response
    assert response.value == {"value": [1, 2]}
    assert response.tail == b"tail"
    assert type(response.tail) is bytes

    child = _internal.map_from_wire(response)["value"]
    assert [item.value for item in _internal.list_from_wire(child)] == [1, 2]
    assert child.tail is response.tail


def test_request_decoder_preserves_explicit_payload_and_tail():
    request = envelope.build_request({"value": np.array([1, 2], dtype=np.int16)})
    decoded = envelope.decode_request(request)
    assert decoded.value["value"] == placeholder(0, 2, "i16")
    assert decoded.tail == np.array([1, 2], dtype="<i2").tobytes()


@pytest.mark.parametrize("status", [1, 2])
def test_error_statuses_require_empty_attachment_tails(status):
    status, response = envelope.parse_envelope(make_envelope(status, {"code": "x", "message": "y"}))
    assert status in (1, 2)
    assert response.tail == b""
    with pytest.raises(ValueError, match="must not contain attachment bytes"):
        envelope.parse_envelope(make_envelope(status, {"code": "x", "message": "y"}, b"x"))


def test_transparent_json_does_not_interpret_exact_attachment_shaped_objects():
    marker = placeholder(0, 8, "f64")
    status, response = envelope.parse_envelope(make_envelope(0, marker, b"not-a-valid-f64-tail"))
    assert status == 0
    assert _internal.json_from_wire(response) == marker

    payload = _internal.json_to_wire({"marker": marker, "__rspyts_json__": marker})
    request = envelope.decode_request(envelope.build_request({"value": payload}))
    assert request.value == {"value": payload}
    assert request.tail == b""


def test_declared_buffer_still_validates_bounds_and_alignment():
    aligned = _internal.Response(placeholder(0, 1, "f64"), np.array([1.5], dtype="<f8").tobytes())
    np.testing.assert_array_equal(_internal.buffer_from_wire(aligned, dtype="f64"), [1.5])

    with pytest.raises(ValueError, match="not aligned"):
        _internal.buffer_from_wire(_internal.Response(placeholder(1, 1, "f64"), b"\0" * 9), dtype="f64")
    with pytest.raises(ValueError, match="exceeds tail length"):
        _internal.buffer_from_wire(_internal.Response(placeholder(0, 2, "f64"), b"\0" * 8), dtype="f64")


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
def test_every_numeric_attachment_dtype_is_little_endian_and_owned(dt, np_dtype, items):
    values = np.array(items, dtype=np_dtype)
    response = _internal.Response(
        placeholder(0, len(items), dt),
        values.astype(values.dtype.newbyteorder("<")).tobytes(),
    )
    decoded = _internal.buffer_from_wire(response, dtype=dt)
    assert decoded.dtype == np.dtype(np_dtype)
    assert decoded.flags.writeable
    assert decoded.flags.owndata
    np.testing.assert_array_equal(decoded, values)


def test_bytes_and_u8_attachments_remain_distinct():
    request = envelope.decode_request(
        envelope.build_request({"bytes": b"\x00\xff", "numeric": np.array([3, 4], dtype=np.uint8)})
    )
    assert request.value["bytes"]["__rspyts_buf__"]["dt"] == "bytes"
    assert request.value["numeric"]["__rspyts_buf__"]["dt"] == "u8"
    assert _internal.bytes_from_wire(request.child(request.value["bytes"])) == b"\x00\xff"
    np.testing.assert_array_equal(
        _internal.buffer_from_wire(request.child(request.value["numeric"]), dtype="u8"),
        [3, 4],
    )


@pytest.mark.parametrize(
    ("body", "match"),
    [
        (None, "malformed buffer placeholder body"),
        ({"off": 0, "len": 1}, "malformed buffer placeholder body"),
        ({"off": -1, "len": 1, "dt": "u8"}, "malformed buffer placeholder"),
        ({"off": True, "len": 1, "dt": "u8"}, "malformed buffer placeholder"),
        ({"off": 0, "len": 1, "dt": "wat"}, "malformed buffer placeholder"),
    ],
)
def test_malformed_declared_attachments_are_rejected(body, match):
    response = _internal.Response({"__rspyts_buf__": body}, b"\0")
    with pytest.raises(ValueError, match=match):
        _internal.buffer_from_wire(response, dtype="u8")


def test_attachment_requires_exact_wrapper_and_expected_dtype():
    with pytest.raises(TypeError, match="attachment wrapper"):
        _internal.buffer_from_wire(_internal.Response({"other": 1}, b""), dtype="u8")
    with pytest.raises(TypeError, match="expected a u8"):
        _internal.buffer_from_wire(_internal.Response(placeholder(0, 1, "i8"), b"\0"), dtype="u8")
    with pytest.raises(ValueError, match="unsupported buffer dtype"):
        _internal.buffer_from_wire(_internal.Response(placeholder(0, 1, "u8"), b"\0"), dtype="f16")


def test_request_encoding_aligns_nested_attachments_and_rejects_coercion():
    request = envelope.decode_request(
        envelope.build_request(
            {
                "prefix": np.array([7], dtype=np.uint8),
                "values": [np.array([1.25, -2.5], dtype=np.float64)],
            }
        )
    )
    first = request.value["prefix"]["__rspyts_buf__"]
    second = request.value["values"][0]["__rspyts_buf__"]
    assert first == {"off": 0, "len": 1, "dt": "u8"}
    assert second["off"] == 8
    assert request.tail[1:8] == b"\0" * 7

    with pytest.raises(TypeError, match="object keys must be exact strings"):
        envelope.build_request({1: "coercible"})  # ty: ignore[invalid-argument-type]
    with pytest.raises(TypeError, match="non-JSON value"):
        envelope.build_request({"value": object()})
    with pytest.raises(TypeError, match="mapping or None"):
        envelope.build_request([1, 2])  # ty: ignore[invalid-argument-type]
    with pytest.raises(TypeError, match="unsupported numpy buffer dtype"):
        envelope.build_request({"value": np.array([1 + 2j])})
    with pytest.raises(ValueError, match="non-finite floats"):
        envelope.build_request({"value": float("nan")})


@pytest.mark.parametrize("length", [0, 1, 4, 8, 11])
def test_short_headers_are_rejected(length):
    with pytest.raises(ValueError, match="truncated envelope header"):
        envelope.parse_envelope(bytes(length))


def test_envelope_status_reserved_lengths_and_json_are_strict():
    response = bytearray(make_envelope(0, None))
    response[0] = 3
    with pytest.raises(ValueError, match="invalid response status"):
        envelope.parse_envelope(bytes(response))

    request = bytearray(envelope.build_request({}))
    request[0] = 1
    with pytest.raises(ValueError, match="invalid request marker"):
        envelope.decode_request(bytes(request))

    response = bytearray(make_envelope(0, None))
    response[2] = 1
    with pytest.raises(ValueError, match="reserved envelope"):
        envelope.parse_envelope(bytes(response))

    response = make_envelope(0, {"a": 1})
    with pytest.raises(ValueError, match="truncated envelope"):
        envelope.parse_envelope(response[:-1])
    with pytest.raises(ValueError, match="trailing bytes"):
        envelope.parse_envelope(response + b"x")

    malformed = bytes([0, 0, 0, 0]) + struct.pack("<II", 1, 0) + b"{"
    with pytest.raises(ValueError, match="malformed envelope JSON"):
        envelope.parse_envelope(malformed)
    nonstandard = bytes([0, 0, 0, 0]) + struct.pack("<II", 3, 0) + b"NaN"
    with pytest.raises(ValueError, match="non-standard JSON number"):
        envelope.parse_envelope(nonstandard)
    with pytest.raises(TypeError, match="owned bytes"):
        envelope.parse_envelope(bytearray(response))  # ty: ignore[invalid-argument-type]


def test_response_rejects_borrowed_or_mutable_tail_context():
    with pytest.raises(TypeError, match="owned bytes"):
        _internal.Response(None, bytearray())  # ty: ignore[invalid-argument-type]
