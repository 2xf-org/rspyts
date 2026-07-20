import numpy as np
from example import RollRequest, roll_values, seed_from_bytes
from example_client import roll_three_dice


def test_generated_python_package() -> None:
    result = roll_three_dice()
    assert result.values == [5, 4, 2]
    assert result.total == 11


def test_generated_binary_types() -> None:
    seed = seed_from_bytes(b"rspyts")
    values = roll_values(RollRequest(sides=6, count=3), seed)
    assert isinstance(values, np.ndarray)
    assert values.dtype == np.uint32
