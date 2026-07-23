import numpy as np
from numpy.typing import ArrayLike

from ..api import summarize_readings


def describe_readings(readings: ArrayLike) -> str:
    """
    Return a compact, human-readable summary of some readings.
    """
    summary = summarize_readings(np.asarray(readings, dtype=np.float64))
    return (
        f"{summary.count} readings: {summary.minimum:.2f} to "
        f"{summary.maximum:.2f} (mean {summary.mean:.2f}, {summary.trend.value})"
    )
