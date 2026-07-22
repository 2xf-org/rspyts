from __future__ import annotations

from pydantic import BaseModel, ConfigDict, Field

import example.dice.summary.models as _rspyts_models_6


class DiceReport(BaseModel):
    """A root report that references a model from a nested namespace."""

    model_config = ConfigDict(
        frozen=True,
        populate_by_name=True,
        extra="forbid",
        arbitrary_types_allowed=True,
    )
    summary: _rspyts_models_6.RollSummary = Field(default=...)


DiceReport.model_rebuild()
