"""Use the package that rspyts generated from the Rust example."""

from example import DEFAULT_SEED, DiceCup, RollResult


def roll_three_dice() -> RollResult:
    """Roll three six-sided dice with the repeatable example seed."""
    cup = DiceCup(6, DEFAULT_SEED)
    try:
        return cup.roll(3)
    finally:
        cup.close()
