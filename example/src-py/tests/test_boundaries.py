import os
import time

import numpy as np

from example import decode_readings, encode_readings, normalize_readings


def test_large_buffers_stay_on_direct_native_boundaries() -> None:
    limit = float(os.environ.get("RSPYTS_PERFORMANCE_LIMIT_SECONDS", "5"))
    readings = np.linspace(-1.0, 1.0, 4 * 1024 * 1024, dtype=np.float64)

    started = time.perf_counter()
    normalized = normalize_readings(readings)
    normalization_seconds = time.perf_counter() - started

    started = time.perf_counter()
    encoded = encode_readings(readings)
    encoding_seconds = time.perf_counter() - started

    started = time.perf_counter()
    decoded = decode_readings(encoded)
    decoding_seconds = time.perf_counter() - started

    assert normalized.dtype == np.float64
    assert normalized.shape == readings.shape
    assert encoded.startswith(b"RSPYTS01")
    assert len(encoded) == readings.nbytes + 12
    assert decoded.dtype == np.float64
    assert decoded.shape == readings.shape
    assert decoded[0] == readings[0]
    assert decoded[-1] == readings[-1]
    assert normalization_seconds < limit
    assert encoding_seconds < limit
    assert decoding_seconds < limit
