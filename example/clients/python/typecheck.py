"""Static contract checks for generated Python namespace facades."""

from typing import assert_type

from example.dice import DiceReport
from example.dice.fair.roll import RollResult
from example.dice.summary import RollSummary, summarize_roll

assert_type(DiceReport, type[DiceReport])
assert_type(RollSummary, type[RollSummary])


def check_facade_types(result: RollResult) -> None:
    assert_type(summarize_roll("checked", result), RollSummary)
