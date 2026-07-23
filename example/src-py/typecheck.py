from typing import assert_type

import numpy as np
from numpy.typing import NDArray

from example import (
    ReadingSummary,
    decode_readings,
    describe_readings,
    encode_readings,
    normalize_readings,
    summarize_readings,
)

readings = np.array([12.0, 18.0, 15.0, 21.0], dtype=np.float64)

assert_type(summarize_readings(readings), ReadingSummary)
assert_type(normalize_readings(readings), NDArray[np.float64])
assert_type(encode_readings(readings), bytes)
assert_type(decode_readings(b"RSPYTS01"), NDArray[np.float64])
assert_type(describe_readings([12.0, 18.0]), str)
