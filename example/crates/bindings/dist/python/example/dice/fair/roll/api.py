from __future__ import annotations

from typing import Final

from pydantic import ConfigDict, TypeAdapter

from .models import (
    RollMode,
    RollRequest,
    RollResult,
    UInt32Buffer,
)
from example.runtime import (
    native,
    native_error,
    prepare_host,
    restore_host,
)


class RollError(RuntimeError):
    """Errors from the fair dice API."""

    def __init__(self, code: str, message: str) -> None:
        super().__init__(message)
        self.code = code


def roll_dice(request: RollRequest, seed: int) -> RollResult:
    """Roll dice from a seed.

    # Errors

    Returns [`RollError::InvalidRequest`] if the request is not valid.
    """
    try:
        native_result = getattr(native, "__rspyts_function_example_dice_3f04e55579ef1c90")(
            prepare_host(request),
            prepare_host(seed),
        )
    except RuntimeError as error:
        raise native_error(error, RollError) from None
    return TypeAdapter(RollResult).validate_python(
        restore_host(native_result, ("named", "example-dice::example_dice::fair::roll::RollResult")),
        strict=False,
    )


def roll_values(request: RollRequest, seed: int) -> UInt32Buffer:
    """Roll dice and return a compact numeric buffer.

    # Errors

    Returns [`RollError::InvalidRequest`] if the request is not valid.
    """
    try:
        native_result = getattr(native, "__rspyts_function_example_dice_79ab58465b7e5143")(
            prepare_host(request),
            prepare_host(seed),
        )
    except RuntimeError as error:
        raise native_error(error, RollError) from None
    return TypeAdapter(
        UInt32Buffer,
        config=ConfigDict(arbitrary_types_allowed=True),
    ).validate_python(
        restore_host(native_result, ("buffer", "uint32")),
        strict=False,
    )


def seed_from_bytes(bytes: bytes) -> int:
    """Convert bytes to a repeatable seed."""
    native_result = getattr(native, "__rspyts_function_example_dice_5026eacb605dfd00")(
        prepare_host(bytes),
    )
    return TypeAdapter(int).validate_python(
        restore_host(native_result, None),
        strict=False,
    )


class DiceCup:
    def __init__(self, sides: int, seed: int) -> None:
        try:
            self.native_resource = getattr(native, "__rspyts_resource_example_dice_e8caefb21754b303")(
                prepare_host(sides),
                prepare_host(seed),
            )
        except RuntimeError as error:
            raise native_error(error, RollError) from None

    def roll(self, count: int) -> RollResult:
        """Roll the dice in this cup.

        # Errors

        Returns [`RollError::InvalidRequest`] if `count` is not from 1 through 100.
        """
        try:
            native_result = self.native_resource.roll(prepare_host(count))
        except RuntimeError as error:
            raise native_error(error, RollError) from None
        return TypeAdapter(RollResult).validate_python(
            restore_host(native_result, ("named", "example-dice::example_dice::fair::roll::RollResult")),
            strict=False,
        )

    def close(self) -> None:
        self.native_resource.close()


DEFAULT_SEED: Final[int] = 42
