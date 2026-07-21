from __future__ import annotations

from pydantic import TypeAdapter

from .models import (
    RollSummary,
)
from example.dice.fair.roll import models as _rspyts_dice__fair__roll__models
from example.dice.fair.roll import api as _rspyts_dice__fair__roll__api
from example.runtime import (
    native,
    native_error,
    prepare_host,
    restore_host,
)


def summarize_roll(
    label: str,
    result: _rspyts_dice__fair__roll__models.RollResult,
) -> RollSummary:
    """Add a label to a fair-roll result.

    # Errors

    Returns [`RollError::EmptyLabel`] if `label` is empty.
    """
    try:
        native_result = getattr(native, "__rspyts_function_example_dice_5e9e146e9f6141a5")(
            prepare_host(label),
            prepare_host(result),
        )
    except RuntimeError as error:
        raise native_error(error, _rspyts_dice__fair__roll__api.RollError) from None
    return TypeAdapter(RollSummary).validate_python(
        restore_host(native_result, ("named", "example-dice::example_dice::summary::RollSummary")),
        strict=False,
    )
