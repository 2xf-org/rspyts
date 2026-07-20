import numpy as np
import pytest
from example.dice.fair.roll import RollError, RollRequest, roll_values, seed_from_bytes
from example.dice.loaded.roll import RollResult as LoadedRollResult, loaded_roll
from example.dice.summary import RollSummary, summarize_roll
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


def test_namespaces_keep_equal_model_names_separate() -> None:
    fair = roll_three_dice()
    loaded: LoadedRollResult = loaded_roll(6)
    summary = summarize_roll("fair", fair)

    assert isinstance(summary, RollSummary)
    assert summary.result.total == 11
    assert loaded.value == 6


def test_cross_namespace_errors_keep_their_public_type() -> None:
    fair = roll_three_dice()

    with pytest.raises(RollError) as raised:
        summarize_roll("", fair)

    assert raised.value.code == "empty_label"
    assert str(raised.value) == "the summary label cannot be empty"
