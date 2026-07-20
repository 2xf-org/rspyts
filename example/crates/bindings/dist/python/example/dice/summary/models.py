from __future__ import annotations

from pydantic import BaseModel, ConfigDict, Field

import example.dice.fair.roll.models


class RollSummary(BaseModel):
    """A named summary of one fair roll."""

    model_config = ConfigDict(
        frozen=True,
        strict=True,
        populate_by_name=True,
        extra="forbid",
        arbitrary_types_allowed=True,
    )
    label: str = Field(default=...)
    result: example.dice.fair.roll.models.RollResult = Field(default=...)


RollSummary.model_rebuild()
