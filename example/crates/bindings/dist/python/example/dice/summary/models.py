from __future__ import annotations

from pydantic import BaseModel, ConfigDict, Field

import example.dice.fair.roll.models as _rspyts_models_3


class RollSummary(BaseModel):
    """A named summary of one fair roll."""

    model_config = ConfigDict(
        frozen=True,
        populate_by_name=True,
        extra="forbid",
        arbitrary_types_allowed=True,
    )
    label: str = Field(default=...)
    result: _rspyts_models_3.RollResult = Field(default=...)


RollSummary.model_rebuild()
