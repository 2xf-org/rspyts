"""
Exact scalar and schemaless-JSON host-type tests.
"""

import pydantic
import pytest

from rspyts import (
    I64,
    U64,
    Contract,
    JsonValue,
    float_from_wire,
    i64_from_wire,
    i64_to_wire,
    json_from_wire,
    u64_from_wire,
    u64_to_wire,
)


class ExactValues(Contract):
    signed: I64
    unsigned: U64


class JsonValues(Contract):
    value: JsonValue


@pytest.mark.parametrize("value", [-(2**63), -(2**53) - 1, -1, 0, 1, 2**63 - 1])
def test_i64_boundaries_and_large_values(value):
    assert i64_from_wire(str(value)) == value
    assert i64_to_wire(value) == str(value)


@pytest.mark.parametrize("value", [0, 1, 2**53 + 1, 2**64 - 1])
def test_u64_boundaries_and_large_values(value):
    assert u64_from_wire(str(value)) == value
    assert u64_to_wire(value) == str(value)


@pytest.mark.parametrize("value", [-(2**63) - 1, 2**63])
def test_i64_overflow_is_rejected(value):
    with pytest.raises(ValueError, match="i64 value out of range"):
        i64_to_wire(value)


@pytest.mark.parametrize("value", [-1, 2**64])
def test_u64_negative_and_overflow_are_rejected(value):
    with pytest.raises(ValueError, match="u64 value out of range"):
        u64_to_wire(value)


@pytest.mark.parametrize("value", ["01", "-0", "+1", "1.0", " 1"])
def test_noncanonical_strings_are_rejected(value):
    with pytest.raises(ValueError, match="canonical"):
        i64_from_wire(value)


@pytest.mark.parametrize("value", [True, False, 1.0, None, object()])
def test_nonintegers_are_rejected(value):
    with pytest.raises(TypeError, match="64-bit integer"):
        u64_to_wire(value)


def test_wire_integer_decoders_require_strings_but_models_accept_host_ints():
    with pytest.raises(TypeError, match="64-bit integer"):
        i64_from_wire(1)
    with pytest.raises(TypeError, match="64-bit integer"):
        u64_from_wire(1)
    assert ExactValues.model_validate({"signed": 1, "unsigned": 2}).model_dump() == {
        "signed": "1",
        "unsigned": "2",
    }


def test_structured_floats_are_finite_and_canonicalize_negative_zero():
    assert float_from_wire(1.25) == 1.25
    assert str(float_from_wire(-0.0)) == "0.0"
    for value in [float("nan"), float("inf"), "1", None, True]:
        with pytest.raises((TypeError, ValueError), match="finite JSON number"):
            float_from_wire(value)


def test_json_alias_wraps_marker_shaped_content_without_interpreting_it():
    marker = {"__rspyts_buf__": {"off": 0, "len": 1, "dt": "u8"}}
    value = JsonValues(value={"marker": marker, "nested": [{"marker": marker}]})
    assert value.model_dump(mode="python") == {
        "value": {
            "__rspyts_json__": {
                "marker": marker,
                "nested": [{"marker": marker}],
            }
        }
    }
    assert json_from_wire({"__rspyts_json__": marker}) == marker
    with pytest.raises(TypeError, match="Json wrapper"):
        json_from_wire(marker)


def test_pydantic_aliases_expose_ints_and_dump_canonical_strings():
    values = ExactValues.model_validate({"signed": "-9007199254740993", "unsigned": 18_446_744_073_709_551_615})
    assert values.signed == -9_007_199_254_740_993
    assert values.unsigned == 18_446_744_073_709_551_615
    assert isinstance(values.signed, int)
    assert values.model_dump(mode="python") == {
        "signed": "-9007199254740993",
        "unsigned": "18446744073709551615",
    }


def test_pydantic_aliases_enforce_bounds():
    with pytest.raises(pydantic.ValidationError, match="u64 value out of range"):
        ExactValues.model_validate({"signed": 0, "unsigned": -1})
