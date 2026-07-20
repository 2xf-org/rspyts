from __future__ import annotations

from datetime import date, datetime
from typing import Any, Final

from pydantic import BaseModel, ConfigDict, TypeAdapter
import numpy as np

from .models import RollRequest, RollResult, UInt32Buffer
from . import native as native


def prepare_host(value: Any) -> Any:
    if isinstance(value, BaseModel):
        return prepare_host(value.model_dump(mode="python", by_alias=True))
    if isinstance(value, (datetime, date)):
        return value.isoformat()
    if isinstance(value, bytes):
        return list(value)
    if "np" in globals() and isinstance(value, np.ndarray):
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
        return tuple(restore_host(item, item_spec) for item, item_spec in zip(value, spec[1]))
    if kind == "named":
        return restore_host(value, native_schemas.get(spec[1]))
    if kind == "alias":
        return restore_host(value, spec[1])
    if kind == "struct":
        return {key: restore_host(item, spec[1].get(key)) for key, item in value.items()}
    if kind == "tagged":
        fields = spec[2].get(value.get(spec[1]), {})
        return {key: restore_host(item, fields.get(key)) for key, item in value.items()}
    return value

def native_error(error: RuntimeError, error_type: type[RuntimeError]) -> RuntimeError:
    if len(error.args) == 2:
        return error_type(str(error.args[0]), str(error.args[1]))
    return error

native_schemas: dict[str, Any] = {
    "RollRequest": ("struct", {"sides": None, "count": None}),
    "RollResult": ("struct", {"values": ("list", None), "total": None}),
}

class RollError(RuntimeError):
    "Errors from the example API."
    def __init__(self, code: str, message: str) -> None:
        super().__init__(message)
        self.code = code


def roll_dice(request: RollRequest, seed: int) -> RollResult:
    "Roll dice from a seed.\nThe seed makes the example repeatable in Rust, Python, and TypeScript.\n# Errors\nReturns [`RollError::InvalidRequest`] when the request is outside the supported ranges."
    try:
        result = native.rollDice(prepare_host(request), prepare_host(seed))
    except RuntimeError as error:
        raise native_error(error, RollError) from None
    return TypeAdapter(RollResult).validate_python(restore_host(result, ("named", "RollResult")))

def roll_values(request: RollRequest, seed: int) -> UInt32Buffer:
    "Roll dice and return a compact numeric buffer.\n# Errors\nReturns [`RollError::InvalidRequest`] when the request is outside the supported ranges."
    try:
        result = native.rollValues(prepare_host(request), prepare_host(seed))
    except RuntimeError as error:
        raise native_error(error, RollError) from None
    return TypeAdapter(UInt32Buffer, config=ConfigDict(arbitrary_types_allowed=True)).validate_python(restore_host(result, ("buffer", "uint32")))

def seed_from_bytes(bytes: bytes) -> int:
    "Convert bytes to a repeatable seed."
    result = native.seedFromBytes(prepare_host(bytes))
    return TypeAdapter(int).validate_python(restore_host(result, None))

class DiceCup:
    def __init__(self, sides: int, seed: int) -> None:
        try:
            self.native_resource = native.DiceCup(prepare_host(sides), prepare_host(seed))
        except RuntimeError as error:
            raise native_error(error, RollError) from None

    def roll(self, count: int) -> RollResult:
        "Roll the dice in this cup.\n# Errors\nReturns [`RollError::InvalidRequest`] when `count` is outside 1 through 100."
        try:
            result = self.native_resource.roll(prepare_host(count))
        except RuntimeError as error:
            raise native_error(error, RollError) from None
        return TypeAdapter(RollResult).validate_python(restore_host(result, ("named", "RollResult")))

    def close(self) -> None:
        self.native_resource.close()

DEFAULT_SEED: Final[int] = 42
