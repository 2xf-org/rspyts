"""
Python surface of the shared-types crate (rspyts multi-crate example).

Notes:
    Everything under ``generated`` is produced by ``rspyts generate`` from
    the Rust crate next door; this package just re-exports it. Downstream
    bridged crates (see ``examples/multi-crate/app``) import these classes
    from here instead of re-emitting them, so a ``Point`` is the same
    class everywhere.
"""

from . import generated
from .generated import Axis, Point

__all__ = [
    "Axis",
    "Point",
    "generated",
]
