from __future__ import annotations

from pydantic import BaseModel, ConfigDict, Field


class RollResult(BaseModel):
    """The result of one loaded-die roll."""

    model_config = ConfigDict(
        frozen=True,
        strict=True,
        populate_by_name=True,
        extra="forbid",
        arbitrary_types_allowed=True,
    )
    value: int = Field(default=...)
    favored_value: int = Field(default=..., alias="favoredValue")


RollResult.model_rebuild()
