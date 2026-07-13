"""
Python surface of the rspyts basic example.

Notes:
    Everything under ``generated`` is produced by ``rspyts generate`` from
    the Rust crate next door; this package just re-exports it. Hand-written
    helpers would live here, importing from the generated modules — never
    the reverse.
"""

from . import generated
from .generated import (
    MAX_WINDOW,
    ROUNDING_MODES,
    BasicError,
    BasicErrorEmptyInput,
    BasicErrorNotANumber,
    BasicErrorTooManyValues,
    BasicErrorUnreadableFile,
    BasicErrorZeroFactor,
    Counter,
    ParsedNumber,
    ParsedNumberDecimal,
    ParsedNumberInteger,
    Rounding,
    Summary,
    annotate,
    load_values,
    parse_number,
    round_value,
    scale,
    simulate_panic,
    summarize,
)

__all__ = [
    "MAX_WINDOW",
    "ROUNDING_MODES",
    "BasicError",
    "BasicErrorEmptyInput",
    "BasicErrorNotANumber",
    "BasicErrorTooManyValues",
    "BasicErrorUnreadableFile",
    "BasicErrorZeroFactor",
    "Counter",
    "ParsedNumber",
    "ParsedNumberDecimal",
    "ParsedNumberInteger",
    "Rounding",
    "Summary",
    "annotate",
    "generated",
    "load_values",
    "parse_number",
    "round_value",
    "scale",
    "simulate_panic",
    "summarize",
]
