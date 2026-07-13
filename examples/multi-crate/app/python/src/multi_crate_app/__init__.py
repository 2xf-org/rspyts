"""
Python surface of the multi-crate-app crate (rspyts multi-crate example).

Notes:
    Everything under ``generated`` is produced by ``rspyts generate`` from
    the Rust crate next door; this package just re-exports it. ``Point``
    and ``Axis`` are not re-emitted copies — the ``[python.imports]`` table
    in ``rspyts.toml`` makes the generated code import them straight from
    ``shared_types``, so they are the very same classes.
"""

from . import generated
from .generated import Axis, Point, mirror, translate

__all__ = [
    "Axis",
    "Point",
    "generated",
    "mirror",
    "translate",
]
