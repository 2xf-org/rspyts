from __future__ import annotations

from datetime import datetime, timezone

import numpy as np
import pytest
from pydantic import ValidationError

from rspyts_acceptance import (
    AnalyzeError,
    Analyzer,
    Channel,
    ContractRequest,
    Quality,
    ResourceClosedError,
    summarize,
    validate_request,
)


def test_function_preserves_types_buffers_and_bytes() -> None:
    result = summarize(
        Channel(id="c3", sample_rate_hz=256),
        np.array([1.0, 2.0, 3.0], dtype=np.float64),
        b"\x01\x02\x03\x04",
        2.0,
    )

    assert result.count == 3
    assert result.average == 2.0
    assert isinstance(result.quality, Quality)
    assert result.quality.value == "good"
    assert isinstance(result.normalized, np.ndarray)
    np.testing.assert_array_equal(result.normalized, np.array([2.0, 4.0, 6.0]))
    assert result.fingerprint == b"\x01\x02\x03\x04"


def test_typed_error_and_resource_lifecycle() -> None:
    with pytest.raises(AnalyzeError) as failure:
        summarize(
            Channel(id="c3", sample_rate_hz=256),
            np.array([], dtype=np.float64),
            b"\x01\x02\x03\x04",
            1.0,
        )
    assert failure.value.code == "empty"

    with Analyzer(Channel(id="c3", sample_rate_hz=256), 0.5) as analyzer:
        result = analyzer.summarize(
            np.array([2.0, 4.0], dtype=np.float64), b"\x09\x08\x07\x06"
        )
        assert analyzer.calls() == 1
        assert isinstance(result.normalized, np.ndarray)
        np.testing.assert_array_equal(result.normalized, np.array([1.0, 2.0]))

    analyzer.close()
    with pytest.raises(ResourceClosedError):
        analyzer.calls()


def test_defaults_constraints_and_aware_datetimes() -> None:
    occurred_at = datetime(2026, 7, 16, 12, 34, 56, tzinfo=timezone.utc)
    request = ContractRequest(
        contract_version=2,
        actor="viewer",
        occurred_at=occurred_at,
        tags=["night"],
    )

    assert request.quantity == 1
    result = validate_request(request)
    assert result.quantity == 1
    assert result.occurred_at == occurred_at

    invalid_values = (
        {"contract_version": 3},
        {"actor": ""},
        {"quantity": 0},
        {"occurred_at": datetime(2026, 7, 16, 12, 34, 56)},
        {"tags": []},
    )
    valid = {
        "contract_version": 2,
        "actor": "viewer",
        "occurred_at": occurred_at,
        "tags": ["night"],
    }
    for override in invalid_values:
        with pytest.raises(ValidationError):
            ContractRequest(**(valid | override))
