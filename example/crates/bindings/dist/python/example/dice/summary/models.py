from __future__ import annotations

from pydantic import BaseModel, ConfigDict, Field

from example.dice.fair.roll import models as _rspyts_dice__fair__roll__models


class RollSummary(BaseModel):
    """A named summary of one fair roll."""

    model_config = ConfigDict(
        frozen=True,
        populate_by_name=True,
        extra="forbid",
        arbitrary_types_allowed=True,
    )
    label: str = Field(default=...)
    result: _rspyts_dice__fair__roll__models.RollResult = Field(default=...)


RollSummary.model_rebuild()
