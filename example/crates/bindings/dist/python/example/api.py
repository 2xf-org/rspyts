from __future__ import annotations

from typing import Final

from pydantic import ConfigDict, TypeAdapter

from .models import (
    RollRequest,
    RollResult,
    UInt32Buffer,
)
from .runtime import (
    native,
    native_error,
    prepare_host,
    restore_host,
)


class RollError(RuntimeError):
    """Errors from the example API."""

    def __init__(self, code: str, message: str) -> None:
        super().__init__(message)
        self.code = code


def roll_dice(request: RollRequest, seed: int) -> RollResult:
    """Roll dice from a seed.

    The seed makes the example repeatable in Rust, Python, and TypeScript.

    # Errors

    Returns [`RollError::InvalidRequest`] when the request is outside the supported
    ranges.
    """
    try:
        result = native.rollDice(prepare_host(request), prepare_host(seed))
    except RuntimeError as error:
        raise native_error(error, RollError) from None
    return TypeAdapter(RollResult).validate_python(
        restore_host(result, ("named", "RollResult")),
        strict=False,
    )


def roll_values(request: RollRequest, seed: int) -> UInt32Buffer:
    """Roll dice and return a compact numeric buffer.

    # Errors

    Returns [`RollError::InvalidRequest`] when the request is outside the supported
    ranges.
    """
    try:
        result = native.rollValues(prepare_host(request), prepare_host(seed))
    except RuntimeError as error:
        raise native_error(error, RollError) from None
    return TypeAdapter(
        UInt32Buffer,
        config=ConfigDict(arbitrary_types_allowed=True),
    ).validate_python(
        restore_host(result, ("buffer", "uint32")),
        strict=False,
    )


def seed_from_bytes(bytes: bytes) -> int:
    """Convert bytes to a repeatable seed."""
    result = native.seedFromBytes(prepare_host(bytes))
    return TypeAdapter(int).validate_python(
        restore_host(result, None),
        strict=False,
    )


class DiceCup:
    def __init__(self, sides: int, seed: int) -> None:
        try:
            self.native_resource = native.DiceCup(
                prepare_host(sides),
                prepare_host(seed),
            )
        except RuntimeError as error:
            raise native_error(error, RollError) from None

    def roll(self, count: int) -> RollResult:
        """Roll the dice in this cup.

        # Errors

        Returns [`RollError::InvalidRequest`] when `count` is outside 1 through 100.
        """
        try:
            result = self.native_resource.roll(prepare_host(count))
        except RuntimeError as error:
            raise native_error(error, RollError) from None
        return TypeAdapter(RollResult).validate_python(
            restore_host(result, ("named", "RollResult")),
            strict=False,
        )

    def close(self) -> None:
        self.native_resource.close()


DEFAULT_SEED: Final[int] = 42
