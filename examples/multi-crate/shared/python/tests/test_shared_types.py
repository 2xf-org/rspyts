"""
Smoke tests for the shared-types Python package.

Notes:
    shared-types is types-only — pydantic models, no native calls — so
    this suite needs no compiled library. It pins the public import
    surface and the wire behavior downstream packages build on.
"""

import pydantic
import pytest

import shared_types


class TestPoint:
    def test_constructs_and_dumps_wire_shape(self):
        point = shared_types.Point(x=1.0, y=2.0)
        assert (point.x, point.y) == (1.0, 2.0)
        assert point.model_dump(by_alias=True, mode="json") == {"x": 1.0, "y": 2.0}

    def test_validates_from_wire_dict(self):
        point = shared_types.Point.model_validate({"x": 3.0, "y": -4.5})
        assert (point.x, point.y) == (3.0, -4.5)

    def test_unknown_fields_are_rejected(self):
        with pytest.raises(pydantic.ValidationError):
            shared_types.Point.model_validate({"x": 1.0, "y": 2.0, "z": 3.0})


class TestAxis:
    def test_wire_values_are_camel_case_variant_names(self):
        assert shared_types.Axis.HORIZONTAL.value == "horizontal"
        assert shared_types.Axis.VERTICAL.value == "vertical"

    def test_reexport_is_the_generated_class(self):
        assert shared_types.Axis is shared_types.generated.models.Axis
        assert shared_types.Point is shared_types.generated.models.Point
