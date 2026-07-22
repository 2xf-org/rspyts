"""Generated from the Rust application API."""

from .api import (
    DEFAULT_FAVORED_VALUE as DEFAULT_FAVORED_VALUE,
    DiceCup as DiceCup,
    roll_dice as roll_dice,
)
from .models import (
    RollResult as RollResult,
)

__all__ = [
    "RollResult",
    "DEFAULT_FAVORED_VALUE",
    "DiceCup",
    "roll_dice",
]
