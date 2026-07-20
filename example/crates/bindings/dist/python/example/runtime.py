from __future__ import annotations

from datetime import date, datetime
from typing import Any

import numpy as np
from pydantic import BaseModel

from . import native as native  # type: ignore[attr-defined]


def prepare_host(value: Any) -> Any:
    if isinstance(value, BaseModel):
        return prepare_host(value.model_dump(mode="python", by_alias=True))
    if isinstance(value, (datetime, date)):
        return value.isoformat()
    if isinstance(value, bytes):
        return list(value)
    if isinstance(value, np.ndarray):
        return value.tolist()
    if isinstance(value, dict):
        return {key: prepare_host(item) for key, item in value.items()}
    if isinstance(value, (list, tuple)):
        return [prepare_host(item) for item in value]
    return value


def restore_host(value: Any, spec: Any) -> Any:
    if value is None or spec is None:
        return value
    kind = spec[0]
    if kind == "bytes":
        return bytes(value)
    if kind == "buffer":
        return np.asarray(value, dtype=spec[1])
    if kind == "list":
        return [restore_host(item, spec[1]) for item in value]
    if kind == "map":
        return {key: restore_host(item, spec[1]) for key, item in value.items()}
    if kind == "tuple":
        return tuple(
            restore_host(item, item_spec) for item, item_spec in zip(value, spec[1])
        )
    if kind == "named":
        return restore_host(value, native_schemas.get(spec[1]))
    if kind == "alias":
        return restore_host(value, spec[1])
    if kind == "struct":
        return {
            key: restore_host(item, spec[1].get(key)) for key, item in value.items()
        }
    if kind == "tagged":
        fields = spec[2].get(value.get(spec[1]), {})
        return {key: restore_host(item, fields.get(key)) for key, item in value.items()}
    return value


def native_error(error: RuntimeError, error_type: type[RuntimeError]) -> RuntimeError:
    if len(error.args) == 2:
        return error_type(str(error.args[0]), str(error.args[1]))
    return error


native_schemas: dict[str, Any] = {
    "example-dice::example_dice::fair::roll::RollRequest": ("struct", {"sides": None, "count": None}),
    "example-dice::example_dice::fair::roll::RollResult": ("struct", {"values": ("list", None), "total": None}),
    "example-dice::example_dice::loaded::roll::RollResult": ("struct", {"value": None, "favoredValue": None}),
    "example-dice::example_dice::summary::RollSummary": ("struct", {"label": None, "result": ("named", "example-dice::example_dice::fair::roll::RollResult")}),
}
