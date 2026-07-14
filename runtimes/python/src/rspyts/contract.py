"""
Pydantic contracts shared by generated rspyts models.
"""

from __future__ import annotations

import pydantic

__all__ = ["Contract", "to_camel"]


def to_camel(name: str) -> str:
    """
    Convert a snake_case identifier to its camelCase wire name.

    Notes:
        The first ``_``-separated part is kept as-is; every following part
        has only its first letter uppercased (``minimum_value`` ->
        ``minimumValue``). Written here rather than borrowed from pydantic
        so the mapping is under rspyts' control and pinned by its own
        tests.

    Args:
        name: The snake_case identifier.

    Returns:
        The camelCase wire name.
    """
    head, *rest = name.split("_")
    return head + "".join(part[:1].upper() + part[1:] for part in rest)


class Contract(pydantic.BaseModel):
    """
    Base class for every generated rspyts model.

    Notes:
        - ``alias_generator=to_camel`` + ``populate_by_name=True``: fields
          are snake_case in Python, camelCase on the wire, constructible
          from either.
        - ``extra="forbid"``: unknown wire fields are rejected, mirroring
          Rust's ``deny_unknown_fields`` — wire compatibility is explicit,
          never accidental.
        - ``arbitrary_types_allowed=True``: generated models may carry
          ``numpy.ndarray`` fields for ``Buf<T>`` returns.
    """

    model_config = pydantic.ConfigDict(
        alias_generator=to_camel,
        populate_by_name=True,
        extra="forbid",
        arbitrary_types_allowed=True,
    )
