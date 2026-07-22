import numpy as np
import pytest
from example.dice import DiceReport
from example.dice.fair.roll import RollError, RollMode, RollRequest, roll_values, seed_from_bytes
from example.dice.loaded.roll import (
    DiceCup as LoadedDiceCup,
    RollResult as LoadedRollResult,
    roll_dice as loaded_roll_dice,
)
from example.dice.summary import RollSummary, summarize_roll
from example.convenience import average_roll
from example_client import roll_three_dice


def test_generated_binary_types() -> None:
    seed = seed_from_bytes(b"rspyts")
    values = roll_values(RollRequest(sides=6, count=3), seed)
    assert isinstance(values, np.ndarray)
    assert values.dtype == np.uint32


def test_generated_string_enums_are_runtime_values() -> None:
    assert RollMode.Safe == "safe"


def test_authored_python_extensions_live_beside_generated_bindings() -> None:
    assert average_roll(roll_three_dice()) == pytest.approx(11 / 3)


def test_namespaces_keep_equal_model_names_separate() -> None:
    fair = roll_three_dice()
    loaded: LoadedRollResult = loaded_roll_dice(6)
    summary = summarize_roll("fair", fair)

    assert isinstance(summary, RollSummary)
    assert summary.result.total == 11
    assert loaded.value == 6


def test_nested_model_packages_import_during_root_facade_initialization() -> None:
    summary = summarize_roll("fair", roll_three_dice())
    report = DiceReport(summary=summary)

    assert report.summary == summary
    assert DiceReport.model_json_schema()["title"] == "DiceReport"


def test_namespace_entrypoints_reexport_generated_all() -> None:
    import example.dice.summary as summary
    from example.dice.summary import api, models

    assert summary.__all__ == [*models.__all__, *api.__all__]
    assert summary.__all__ == ["RollSummary", "summarize_roll"]


def test_namespaces_keep_equal_function_and_resource_names_separate() -> None:
    fair = roll_three_dice()
    loaded = loaded_roll_dice(4)
    cup = LoadedDiceCup(6)
    try:
        from_cup = cup.roll(3)
    finally:
        cup.close()

    assert fair.total == 11
    assert loaded.value == 4
    assert from_cup.value == 3
    assert from_cup.favored_value == 6


def test_cross_namespace_errors_keep_their_public_type() -> None:
    fair = roll_three_dice()

    with pytest.raises(RollError) as raised:
        summarize_roll("", fair)

    assert raised.value.code == "empty_label"
    assert str(raised.value) == "the summary label cannot be empty"
