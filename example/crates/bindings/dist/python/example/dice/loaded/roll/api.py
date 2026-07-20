from __future__ import annotations

from typing import Final

from pydantic import TypeAdapter

from .models import (
    RollResult,
)
from example.runtime import (
    native,
    prepare_host,
    restore_host,
)


def loaded_roll(value: int) -> RollResult:
    """Return one loaded-die result."""
    native_result = native.loadedRoll(prepare_host(value))
    return TypeAdapter(RollResult).validate_python(
        restore_host(native_result, ("named", "example-dice::example_dice::loaded::roll::RollResult")),
        strict=False,
    )


DEFAULT_FAVORED_VALUE: Final[int] = 6
