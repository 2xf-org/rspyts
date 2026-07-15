"""Application surface and emitter-facing ABI 3 codec tests."""

import copy
import enum
import math

import numpy as np
import pydantic
import pytest

import rspyts
from rspyts import _internal


def response(value, tail: bytes = b"") -> _internal.Response:
    return _internal.Response(value, tail)


class WireState(enum.StrEnum):
    READY = "ready"


class WireChild(rspyts.Contract):
    active: bool
    ratio: float
    label: str
    sequence: int = pydantic.Field(strict=True, ge=0, le=18_446_744_073_709_551_615)
    state: WireState

    @classmethod
    def _from_wire(cls, raw: _internal.Response):
        value = _internal.map_from_wire(raw)
        converted = {key: item.value for key, item in value.items()}
        converted["active"] = _internal.bool_from_wire(value["active"])
        converted["ratio"] = _internal.float_from_wire(value["ratio"])
        converted["label"] = _internal.string_from_wire(value["label"])
        converted["sequence"] = _internal.u64_from_wire(value["sequence"])
        converted["state"] = WireState(_internal.string_from_wire(value["state"]))
        return cls.model_validate(converted, strict=True)


class WireParent(rspyts.Contract):
    child: WireChild
    children: list[WireChild]

    @classmethod
    def _from_wire(cls, raw: _internal.Response):
        value = _internal.map_from_wire(raw)
        converted = {key: item.value for key, item in value.items()}
        converted["child"] = WireChild._from_wire(value["child"])
        converted["children"] = [WireChild._from_wire(item) for item in _internal.list_from_wire(value["children"])]
        return cls.model_validate(converted, strict=True)


def test_top_level_public_surface_is_small():
    assert set(rspyts.__all__) == {
        "BridgeError",
        "Contract",
        "Library",
        "RspytsPanicError",
        "StaleHandleError",
    }
    assert not hasattr(rspyts, "I64")
    assert not hasattr(rspyts, "U64")
    assert not hasattr(rspyts, "JsonValue")
    assert not hasattr(rspyts, "call_raw")
    assert not hasattr(rspyts, "buffer_from_wire")


def test_emitter_api_has_an_explicit_migration_gate():
    assert _internal.EMITTER_API_VERSION == 3
    assert _internal.require_emitter_api(3) is None
    with pytest.raises(RuntimeError, match="requires emitter API 2"):
        _internal.require_emitter_api(2)


@pytest.mark.parametrize("value", [-(2**63), -(2**53) - 1, -1, 0, 1, 2**63 - 1])
def test_i64_boundaries_and_large_values(value):
    assert _internal.i64_from_wire(response(str(value))) == value
    assert _internal.i64_to_wire(value) == str(value)


@pytest.mark.parametrize("value", [0, 1, 2**53 + 1, 2**64 - 1])
def test_u64_boundaries_and_large_values(value):
    assert _internal.u64_from_wire(response(str(value))) == value
    assert _internal.u64_to_wire(value) == str(value)


@pytest.mark.parametrize("value", [-(2**63) - 1, 2**63])
def test_i64_overflow_is_rejected(value):
    with pytest.raises(ValueError, match="i64 value out of range"):
        _internal.i64_to_wire(value)


@pytest.mark.parametrize("value", [-1, 2**64])
def test_u64_negative_and_overflow_are_rejected(value):
    with pytest.raises(ValueError, match="u64 value out of range"):
        _internal.u64_to_wire(value)


@pytest.mark.parametrize("value", ["01", "-0", "+1", "1.0", " 1"])
def test_noncanonical_wire_integers_are_rejected(value):
    with pytest.raises(ValueError, match="canonical"):
        _internal.i64_from_wire(response(value))


@pytest.mark.parametrize("serializer", [_internal.i64_to_wire, _internal.u64_to_wire])
@pytest.mark.parametrize("value", [True, False, 1.0, "1", "-1", None, object()])
def test_host_integer_serializers_do_not_coerce(serializer, value):
    with pytest.raises(TypeError, match="64-bit integer"):
        serializer(value)


def test_wire_integer_decoders_require_response_and_strings():
    with pytest.raises(TypeError, match="requires a Response"):
        _internal.i64_from_wire("1")  # ty: ignore[invalid-argument-type]
    with pytest.raises(TypeError, match="canonical decimal string"):
        _internal.i64_from_wire(response(1))
    with pytest.raises(TypeError, match="canonical decimal string"):
        _internal.u64_from_wire(response(True))


def test_structured_floats_are_finite_bounded_and_canonical():
    assert _internal.float_from_wire(response(1.25)) == 1.25
    assert str(_internal.float_from_wire(response(-0.0))) == "0.0"
    f32_max = float(np.finfo(np.float32).max)
    assert _internal.float_from_wire(response(f32_max), f32=True) == f32_max
    for value in [float("nan"), float("inf"), "1", None, True]:
        with pytest.raises((TypeError, ValueError), match="finite JSON number"):
            _internal.float_from_wire(response(value))
    for value in [f32_max * 2, -f32_max * 2]:
        with pytest.raises(ValueError, match="finite f32 JSON number"):
            _internal.float_from_wire(response(value), f32=True)


def test_exact_scalar_decoders_do_not_coerce():
    assert _internal.bool_from_wire(response(True)) is True
    assert _internal.bounded_int_from_wire(response(7), minimum=0, maximum=10) == 7
    assert _internal.string_from_wire(response("value")) == "value"
    assert _internal.null_from_wire(response(None)) is None

    for value in [0, 1.0, "true", None]:
        with pytest.raises(TypeError, match="JSON boolean"):
            _internal.bool_from_wire(response(value))
    for value in [True, 1.0, "1", None]:
        with pytest.raises(TypeError, match="JSON integer"):
            _internal.bounded_int_from_wire(response(value), minimum=0, maximum=10)
    with pytest.raises(ValueError, match="out of range"):
        _internal.bounded_int_from_wire(response(11), minimum=0, maximum=10)
    for value in [1, b"value", None]:
        with pytest.raises(TypeError, match="JSON string"):
            _internal.string_from_wire(response(value))
    for value in [False, 0, "null", {}]:
        with pytest.raises(TypeError, match="JSON null"):
            _internal.null_from_wire(response(value))


def test_container_decoders_return_child_responses():
    raw = response({"items": [1, 2]})
    mapped = _internal.map_from_wire(raw)
    items = _internal.list_from_wire(mapped["items"])
    assert [item.value for item in items] == [1, 2]
    assert all(item.tail is raw.tail for item in items)
    assert [item.value for item in _internal.tuple_from_wire(mapped["items"], length=2)] == [1, 2]

    with pytest.raises(TypeError, match="JSON array"):
        _internal.list_from_wire(response((1, 2)))
    with pytest.raises(TypeError, match="JSON object"):
        _internal.map_from_wire(response([("key", 1)]))
    with pytest.raises(TypeError, match="object key"):
        _internal.map_from_wire(response({1: "value"}))
    with pytest.raises(ValueError, match="length 3"):
        _internal.tuple_from_wire(mapped["items"], length=3)


def test_data_enum_dispatch_passes_the_full_response_to_variant_decoder():
    raw = response({"kind": "ready", "value": 3}, b"tail")
    assert _internal.enum_from_wire(
        raw,
        tag="kind",
        variants={"ready": lambda variant: (_internal.map_from_wire(variant)["value"].value, variant.tail)},
    ) == (3, b"tail")
    with pytest.raises(TypeError, match="required enum tag"):
        _internal.enum_from_wire(response({}), tag="kind", variants={})
    with pytest.raises(TypeError, match="JSON string"):
        _internal.enum_from_wire(response({"kind": 1}), tag="kind", variants={})
    with pytest.raises(ValueError, match="unknown 'kind' discriminator"):
        _internal.enum_from_wire(response({"kind": "missing"}), tag="kind", variants={})


def test_generated_style_nested_constructors_reject_scalar_coercion():
    child = {
        "active": True,
        "ratio": 1.25,
        "label": "channel",
        "sequence": str(2**64 - 1),
        "state": "ready",
    }
    result = WireParent._from_wire(response({"child": child, "children": [child]}))
    assert result.child.sequence == 2**64 - 1
    assert result.child.state is WireState.READY
    assert result.children[0].ratio == 1.25

    wrong_values = {"active": 1, "ratio": "1.25", "label": 7, "sequence": 7, "state": 7}
    for field, wrong in wrong_values.items():
        invalid = copy.deepcopy(child)
        invalid[field] = wrong
        with pytest.raises((TypeError, ValueError, pydantic.ValidationError)):
            WireParent._from_wire(response({"child": child, "children": [invalid]}))


def test_json_is_transparent_and_reserved_shapes_are_opaque():
    marker = {"__rspyts_buf__": {"off": 0, "len": 1, "dt": "u8"}}
    value = {
        "marker": marker,
        "nested": [{"__rspyts_json__": marker}],
        "negativeZero": -0.0,
    }
    encoded = _internal.json_to_wire(value)
    decoded = _internal.json_from_wire(response(value, b"ignored tail"))
    assert encoded == value
    assert decoded == value
    assert math.copysign(1.0, encoded["negativeZero"]) == 1.0
    assert math.copysign(1.0, decoded["negativeZero"]) == 1.0


def test_json_values_reject_recursive_coercion_and_cycles():
    valid = {"nested": [None, True, 1, 1.5, "value", {"key": "item"}]}
    checked = _internal.json_to_wire(valid)
    assert checked == valid
    assert checked is not valid
    for invalid in [
        {"nested": [{1: "integer key"}]},
        {"nested": [{"key": ("tuple",)}]},
        {"nested": [{"key": {"set"}}]},
        {"nested": [{"key": b"bytes"}]},
        {"nested": [{"key": object()}]},
        {"nested": [{"key": float("nan")}]},
        {"nested": [{"key": 2**53}]},
        {"nested": [{"key": -(2**53)}]},
        {"nested": [{"key": float(2**53)}]},
        {"nested": [{"key": 1e100}]},
    ]:
        with pytest.raises((TypeError, ValueError), match="schemaless JSON"):
            _internal.json_to_wire(invalid)
        with pytest.raises((TypeError, ValueError), match="schemaless JSON"):
            _internal.json_from_wire(response(invalid))
    cyclic: list[object] = []
    cyclic.append(cyclic)
    with pytest.raises(ValueError, match="reference cycles"):
        _internal.json_to_wire(cyclic)
