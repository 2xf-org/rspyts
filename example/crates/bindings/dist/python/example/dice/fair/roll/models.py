from __future__ import annotations

from typing import Annotated, TypeAlias

import numpy as np
from numpy.typing import NDArray
from pydantic import BaseModel, ConfigDict, Field
from pydantic.functional_serializers import PlainSerializer
from pydantic.functional_validators import BeforeValidator

UInt32Buffer: TypeAlias = Annotated[
    NDArray[np.uint32],
    BeforeValidator(lambda value: np.asarray(value, dtype=np.uint32)),
    PlainSerializer(lambda value: value.tolist(), return_type=list),
]


class RollRequest(BaseModel):
    """A request to roll one type of die."""

    model_config = ConfigDict(
        frozen=True,
        populate_by_name=True,
        extra="forbid",
        arbitrary_types_allowed=True,
    )
    # The number of sides on each die.
    sides: int = Field(default=..., ge=2, le=100)
    # The number of dice to roll.
    count: int = Field(default=..., ge=1, le=100)


class RollResult(BaseModel):
    """The result of a fair dice roll."""

    model_config = ConfigDict(
        frozen=True,
        populate_by_name=True,
        extra="forbid",
        arbitrary_types_allowed=True,
    )
    values: list[int] = Field(default=...)
    total: int = Field(default=...)


RollRequest.model_rebuild()
RollResult.model_rebuild()
