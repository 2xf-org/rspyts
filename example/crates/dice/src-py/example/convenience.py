from .dice.fair.roll import RollResult

__all__ = ["average_roll"]


def average_roll(result: RollResult) -> float:
    """Return the arithmetic mean of a generated roll result."""
    return result.total / len(result.values)
