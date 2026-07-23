from collections.abc import Callable

import numpy as np
import pytest

from example import (
    ReadingError,
    ReadingSummary,
    Trend,
    decode_readings,
    describe_readings,
    encode_readings,
    normalize_readings,
    summarize_readings,
)


def test_generated_api_and_authored_helper_agree() -> None:
    readings = np.array([12.0, 18.0, 15.0, 21.0], dtype=np.float64)

    assert summarize_readings(readings) == ReadingSummary(
        count=4,
        minimum=12.0,
        maximum=21.0,
        mean=16.5,
        trend=Trend.Rising,
    )
    assert describe_readings(readings) == (
        "4 readings: 12.00 to 21.00 (mean 16.50, rising)"
    )


def test_normalization_returns_a_native_float64_array() -> None:
    normalized = normalize_readings(np.array([10.0, 15.0, 20.0], dtype=np.float64))

    assert normalized.dtype == np.float64
    np.testing.assert_array_equal(normalized, np.array([0.0, 0.5, 1.0]))


def test_portable_format_round_trips_between_byte_and_buffer_boundaries() -> None:
    readings = np.array([-1.25, 0.0, 42.5], dtype=np.float64)

    encoded = encode_readings(readings)

    assert isinstance(encoded, bytes)
    assert encoded.startswith(b"RSPYTS01")
    np.testing.assert_array_equal(decode_readings(encoded), readings)


@pytest.mark.parametrize(
    ("operation", "code", "message"),
    [
        (
            lambda: summarize_readings(np.array([], dtype=np.float64)),
            "empty_readings",
            "at least one reading is required",
        ),
        (
            lambda: summarize_readings(np.array([np.nan], dtype=np.float64)),
            "non_finite_reading",
            "readings must contain only finite numbers",
        ),
        (
            lambda: decode_readings(b"not an encoded series"),
            "invalid_encoding",
            "invalid encoded readings",
        ),
    ],
)
def test_domain_errors_have_stable_codes(
    operation: Callable[[], object],
    code: str,
    message: str,
) -> None:
    with pytest.raises(ReadingError) as caught:
        operation()

    assert caught.value.code == code
    assert str(caught.value) == message
