"""
Contract base-model and alias-generator tests (codegen.md §4.1, §7).
"""

import numpy as np
import pytest
from pydantic import ValidationError

from rspyts import Contract, contract


@pytest.mark.parametrize(
    ("snake", "camel"),
    [
        ("minimum_value", "minimumValue"),
        ("batch_size", "batchSize"),
        ("tolerance", "tolerance"),
        ("x", "x"),
        ("a_b_c", "aBC"),
        # Only the first letter of each later part is uppercased.
        ("http_url", "httpUrl"),
        ("value_2d", "value2d"),
        # Edge cases: empty parts vanish, digit-leading parts pass through.
        ("word", "word"),
        ("trailing_", "trailing"),
        ("a__b", "aB"),
        ("point_3d", "point3d"),
    ],
)
def test_to_camel(snake, camel):
    assert contract.to_camel(snake) == camel


class QueryOptions(Contract):
    minimum_value: float
    tolerance: float | None = None


def test_construct_by_python_field_name():
    options = QueryOptions(minimum_value=1.5)
    assert options.minimum_value == 1.5
    assert options.tolerance is None


def test_validate_from_wire_camel_case():
    options = QueryOptions.model_validate({"minimumValue": 1.5, "tolerance": 0.25})
    assert options.minimum_value == 1.5
    assert options.tolerance == 0.25


def test_dump_by_alias_produces_wire_names():
    options = QueryOptions(minimum_value=2.0)
    assert options.model_dump(by_alias=True) == {"minimumValue": 2.0, "tolerance": None}


def test_camel_case_round_trip():
    wire = {"minimumValue": 3.5, "tolerance": 1.0}
    assert QueryOptions.model_validate(wire).model_dump(by_alias=True) == wire


def test_populate_by_name_accepts_snake_case_in_validate():
    options = QueryOptions.model_validate({"minimum_value": 1.5})
    assert options.minimum_value == 1.5


def test_populate_by_name_accepts_alias_in_constructor():
    options = QueryOptions(**{"minimumValue": 1.5})
    assert options.minimum_value == 1.5


def test_unknown_wire_fields_are_rejected():
    with pytest.raises(ValidationError):
        QueryOptions.model_validate({"minimumValue": 1.0, "unknownField": 2})


def test_unknown_constructor_fields_are_rejected():
    with pytest.raises(ValidationError):
        QueryOptions(minimum_value=1.0, unknown_field=2)  # ty: ignore[unknown-argument]


def test_numpy_fields_are_allowed():
    class Report(Contract):
        peak_values: np.ndarray

    report = Report.model_validate({"peakValues": np.array([1.0, 2.0])})
    np.testing.assert_array_equal(report.peak_values, np.array([1.0, 2.0]))
