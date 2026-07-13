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
        ("min_duration_s", "minDurationS"),
        ("sample_rate", "sampleRate"),
        ("threshold", "threshold"),
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


class AnalysisParams(Contract):
    min_duration_s: float
    threshold: float | None = None


def test_construct_by_python_field_name():
    params = AnalysisParams(min_duration_s=1.5)
    assert params.min_duration_s == 1.5
    assert params.threshold is None


def test_validate_from_wire_camel_case():
    params = AnalysisParams.model_validate({"minDurationS": 1.5, "threshold": 0.25})
    assert params.min_duration_s == 1.5
    assert params.threshold == 0.25


def test_dump_by_alias_produces_wire_names():
    params = AnalysisParams(min_duration_s=2.0)
    assert params.model_dump(by_alias=True) == {"minDurationS": 2.0, "threshold": None}


def test_camel_case_round_trip():
    wire = {"minDurationS": 3.5, "threshold": 1.0}
    assert AnalysisParams.model_validate(wire).model_dump(by_alias=True) == wire


def test_populate_by_name_accepts_snake_case_in_validate():
    params = AnalysisParams.model_validate({"min_duration_s": 1.5})
    assert params.min_duration_s == 1.5


def test_populate_by_name_accepts_alias_in_constructor():
    params = AnalysisParams(**{"minDurationS": 1.5})
    assert params.min_duration_s == 1.5


def test_unknown_wire_fields_are_rejected():
    with pytest.raises(ValidationError):
        AnalysisParams.model_validate({"minDurationS": 1.0, "unknownField": 2})


def test_unknown_constructor_fields_are_rejected():
    with pytest.raises(ValidationError):
        AnalysisParams(min_duration_s=1.0, unknown_field=2)


def test_numpy_fields_are_allowed():
    class Report(Contract):
        peak_values: np.ndarray

    report = Report.model_validate({"peakValues": np.array([1.0, 2.0])})
    np.testing.assert_array_equal(report.peak_values, np.array([1.0, 2.0]))
