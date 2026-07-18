from __future__ import annotations

from datetime import datetime, timezone

import numpy as np
import pytest
from pydantic import ValidationError

from example.owner import authored_label
from example.owner.contracts import (
    BatchOptions,
    CalculationError,
    Calculator,
    Item,
    ItemKind,
    Magnitude,
    Quantity,
    ResourceClosedError,
    VectorSpec,
    calculate,
    validate_batch_options,
)


def test_authored_source_is_staged_with_generated_contracts() -> None:
    assert authored_label() == "owner-authored"


def test_function_preserves_types_buffers_and_bytes() -> None:
    result = calculate(
        VectorSpec(name="example", dimensions=3),
        np.array([1.0, 2.0, 3.0], dtype=np.float64),
        b"\x01\x02\x03\x04",
        2.0,
    )

    assert result.count == 3
    assert result.mean == 2.0
    assert isinstance(result.magnitude, Magnitude)
    assert result.magnitude.value == "regular"
    assert isinstance(result.scaled, np.ndarray)
    np.testing.assert_array_equal(result.scaled, np.array([2.0, 4.0, 6.0]))
    assert result.checksum == b"\x01\x02\x03\x04"


def test_typed_error_and_resource_lifecycle() -> None:
    with pytest.raises(CalculationError) as failure:
        calculate(
            VectorSpec(name="example", dimensions=3),
            np.array([], dtype=np.float64),
            b"\x01\x02\x03\x04",
            1.0,
        )
    assert failure.value.code == "empty"

    for checksum in (b"\x01\x02\x03", b"\x01\x02\x03\x04\x05"):
        with pytest.raises(CalculationError) as length_failure:
            calculate(
                VectorSpec(name="example", dimensions=3),
                np.array([1.0], dtype=np.float64),
                checksum,
                1.0,
            )
        assert length_failure.value.code == "invalid_argument"

    with pytest.raises(CalculationError) as boundary_failure:
        calculate(
            VectorSpec(name="example", dimensions=3),
            np.array([1.0], dtype=np.float64),
            b"\x01\x02\x03\x04",
            float("nan"),
        )
    assert boundary_failure.value.code == "invalid_argument"

    with Calculator(VectorSpec(name="example", dimensions=3), 0.5) as calculator:
        result = calculator.calculate(
            np.array([2.0, 4.0], dtype=np.float64), b"\x09\x08\x07\x06"
        )
        assert calculator.calls() == 1
        assert isinstance(result.scaled, np.ndarray)
        np.testing.assert_array_equal(result.scaled, np.array([1.0, 2.0]))

    calculator.close()
    with pytest.raises(ResourceClosedError):
        calculator.calls()


def test_defaults_constraints_and_aware_datetimes() -> None:
    created_at = datetime(2030, 1, 2, 3, 4, 5, tzinfo=timezone.utc)
    options = BatchOptions(
        schema_version=1,
        label="example",
        created_at=created_at,
        groups=["primary"],
    )

    assert options.attempts == 1
    result = validate_batch_options(options)
    assert result.attempts == 1
    assert result.created_at == created_at

    wire_options = BatchOptions.model_validate(
        {
            "schema_version": 1,
            "label": "wire-example",
            "created_at": created_at.isoformat(),
            "groups": ["primary"],
        }
    )
    assert wire_options.created_at == created_at

    wire_item = Item.model_validate(
        {
            "id": "item-1",
            "quantity": {"numerator": 1, "denominator": 1},
            "kind": "standard",
            "tag": b"wire",
        }
    )
    assert wire_item.quantity == Quantity(numerator=1, denominator=1)
    assert wire_item.kind is ItemKind.Standard

    invalid_values = (
        {"schema_version": 2},
        {"schema_version": 1.0},
        {"schema_version": True},
        {"schema_version": "1"},
        {"label": ""},
        {"attempts": 0},
        {"attempts": 4},
        {"attempts": True},
        {"attempts": "2"},
        {"attempts": 2.0},
        {"created_at": datetime(2030, 1, 2, 3, 4, 5)},
        {"created_at": 0},
        {"groups": []},
    )
    valid = {
        "schema_version": 1,
        "label": "example",
        "created_at": created_at,
        "groups": ["primary"],
    }
    for override in invalid_values:
        with pytest.raises(ValidationError):
            BatchOptions(**(valid | override))

    with pytest.raises(ValidationError):
        Item.model_validate(
            {
                "id": "item-1",
                "quantity": {"numerator": 1, "denominator": 1},
                "kind": b"standard",
                "tag": b"wire",
            }
        )
