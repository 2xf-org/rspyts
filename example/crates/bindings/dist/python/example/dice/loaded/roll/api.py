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


def roll_dice(value: int) -> RollResult:
    """Return one loaded-die result."""
    native_result = getattr(native, "__rspyts_function_example_dice_3af246b4f17bbfac")(
        prepare_host(value),
    )
    return TypeAdapter(RollResult).validate_python(
        restore_host(native_result, ("named", "example-dice::example_dice::loaded::roll::RollResult")),
        strict=False,
    )


class DiceCup:
    def __init__(self, favored_value: int) -> None:
        self.native_resource = getattr(native, "__rspyts_resource_example_dice_e5e96c8d1c3073b6")(
            prepare_host(favored_value),
        )

    def roll(self, value: int) -> RollResult:
        """Return one loaded-die result."""
        native_result = self.native_resource.roll(prepare_host(value))
        return TypeAdapter(RollResult).validate_python(
            restore_host(native_result, ("named", "example-dice::example_dice::loaded::roll::RollResult")),
            strict=False,
        )

    def close(self) -> None:
        self.native_resource.close()


DEFAULT_FAVORED_VALUE: Final[int] = 6
