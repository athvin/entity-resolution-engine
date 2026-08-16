"""``std_hash``, the T-STD-1 digest over ``int_std_records`` (S8.3, S5.0, S4.2).

M7 is that "byte-identical `std_records`" was asserted as the standardization
determinism guarantee and is unimplementable: Parquet output is not reproducible,
and the relation carries ``VOLATILE_COLUMNS`` — the ingest stamps differ between
two runs that produced identical data. T-STD-1 redefines determinism as a **content
hash over an explicit stable column list**, and :func:`table_content_hash` is that
hash. Byte identity of the data files is asserted by nothing, deliberately.

The column list is :data:`STD_HASH_COLUMNS`, the literal T-STD-1 order of S8.3. Its
disjointness from ``VOLATILE_COLUMNS`` and its containment in ``STD_RECORD_COLUMNS``
are checked *at import*, against the frozen sets in :mod:`er.lake.columns` rather
than against a copy — a determinism comparison that hashed a run-scoped stamp would
fail on every second run for a reason that has nothing to do with standardization
(S5.0, M7).

The digest itself is :func:`er.ingest.hashing.content_hash`, unchanged: values joined
by the ``0x1f`` unit separator, NULL encoded as the empty string, UTF-8, SHA-256,
lowercase hex. S4.1's ``content_hash`` and this function are different functions with
different column lists — that distinction is real and is why this module exists — but
they are the same *digest*, and spelling the join and the hash a second time is how
the two encodings would drift apart. What is new here is the rendering step in front
of it: ``int_std_records`` holds BOOLEAN, DATE, TIMESTAMP and ``LIST(VARCHAR)``
columns, so the row must be reduced to text before it can be joined, and that
reduction is pinned by :func:`_render` and by a committed golden vector.
"""

from __future__ import annotations

from collections.abc import Mapping, Sequence
from datetime import date, datetime
from typing import Final

from er.ingest.hashing import UNIT_SEPARATOR, content_hash
from er.lake.columns import STD_RECORD_COLUMNS, VOLATILE_COLUMNS

__all__ = ["STD_HASH_COLUMNS", "table_content_hash"]

#: The T-STD-1 projection of S8.3, in its literal order. It is not sorted, it is not
#: derived from ``STD_RECORD_COLUMNS``, and it may not be reordered: the order is the
#: preimage, so changing it changes every digest and invalidates every committed
#: ``expected/<phase>/std_hashes.csv`` (S8.2.1). ``name_variants`` is a member under
#: its own name; S8.3 hashes it as ``array_to_string(name_variants,'\x1f')``, which is
#: a rendering rule and not a different column.
STD_HASH_COLUMNS: Final[tuple[str, ...]] = (
    "record_key",
    "std_version",
    "given_name",
    "family_name",
    "name_variants",
    "email",
    "email_valid",
    "phone_e164",
    "phone_valid",
    "addr_number",
    "addr_street",
    "addr_unit",
    "addr_city",
    "addr_region",
    "addr_postal",
    "birth_date",
    "updated_at_source",
    "content_hash",
)

# The two properties S5.0 requires of that list, checked where they cannot be skipped.
# `tests/unit/lake/test_hashing.py` asserts both as well; this runs at import so that a
# reordering edit that never reaches the unit layer -- a fixture script, a REPL, the
# golden assembler -- still fails at the point of use rather than producing a digest
# nothing can reproduce.
_VOLATILE_MEMBERS: Final[tuple[str, ...]] = tuple(
    column for column in STD_HASH_COLUMNS if column in VOLATILE_COLUMNS
)
if _VOLATILE_MEMBERS:
    raise ImportError(
        f"STD_HASH_COLUMNS names run-scoped columns {_VOLATILE_MEMBERS}: a determinism "
        f"comparison over them can never be stable (S5.0, M7)"
    )

_UNKNOWN_MEMBERS: Final[tuple[str, ...]] = tuple(
    column for column in STD_HASH_COLUMNS if column not in STD_RECORD_COLUMNS
)
if _UNKNOWN_MEMBERS:
    raise ImportError(
        f"STD_HASH_COLUMNS names {_UNKNOWN_MEMBERS}, which int_std_records does not "
        f"declare (S5); the T-STD-1 projection is a subset of STD_RECORD_COLUMNS"
    )


def _render(column: str, value: object) -> str:
    """Render one ``int_std_records`` value as the text T-STD-1 hashes.

    One rule per S5 type on this relation, and no fallback. ``str(value)`` for an
    unrecognised type would silently absorb a schema change into the preimage — a new
    column type, or a driver that starts handing back ``Decimal`` — and the digest it
    produced would differ from every committed one for a reason no failure message
    would name.

    * NULL is the empty string, which is deliberately indistinguishable from an
      empty-string value. S8.3 pins that, and it MUST NOT be given a sentinel: the
      committed hashes were authored under this encoding.
    * BOOLEAN is ``true`` / ``false``, DuckDB's own text rendering of the flags, and
      checked before anything else that could claim an ``int``.
    * TIMESTAMP renders before DATE because :class:`~datetime.datetime` is a subclass
      of :class:`~datetime.date`; taking the DATE branch would truncate
      ``updated_at_source`` to its day and hash two distinct instants alike.
    * ``LIST(VARCHAR)`` is ``array_to_string(<list>,'\\x1f')``: the elements joined by
      the same separator that joins the columns. S8.3 spells exactly this for
      ``name_variants``, ambiguity with a column boundary included.
    """
    if value is None:
        return ""
    if isinstance(value, str):
        return value
    if isinstance(value, bool):
        return "true" if value else "false"
    if isinstance(value, datetime):
        return value.isoformat(sep=" ")
    if isinstance(value, date):
        return value.isoformat()
    if isinstance(value, list | tuple):
        return _render_list(column, value)
    raise TypeError(
        f"{column}: cannot render {type(value).__name__} into the T-STD-1 preimage; "
        f"int_std_records declares VARCHAR, BOOLEAN, DATE, TIMESTAMP and LIST(VARCHAR) (S5)"
    )


def _render_list(column: str, value: Sequence[object]) -> str:
    """``array_to_string(value, '\\x1f')`` — the S8.3 rendering of ``name_variants``."""
    for element in value:
        if not isinstance(element, str):
            raise TypeError(
                f"{column}: LIST(VARCHAR) holds {type(element).__name__}; "
                f"array_to_string has no rendering for it (S5)"
            )
    return UNIT_SEPARATOR.join(str(element) for element in value)


def table_content_hash(row: Mapping[str, object]) -> str:
    """Return the T-STD-1 ``std_hash`` of one ``int_std_records`` row (S8.3).

    ``row`` maps column name to value as the driver returns it. Every member of
    :data:`STD_HASH_COLUMNS` must be present; extra keys — the ingest stamps, or any
    other ``VOLATILE_COLUMNS`` member the caller selected — are ignored, which is what
    makes two rows differing only in a volatile value hash identically.

    A *missing* member is refused rather than treated as NULL. S4.1's ``content_hash``
    reads a source row, where an absent optional column is ordinary; here the row comes
    from a contracted relation whose column list S5 fixes, so an absent column is a
    projection bug, and hashing it as NULL would make a short projection agree with a
    complete one whose value happened to be NULL.

    The hash is per row: no ordering, no aggregation, and no dependence on any other
    row of the relation.
    """
    missing = tuple(column for column in STD_HASH_COLUMNS if column not in row)
    if missing:
        raise KeyError(
            f"row is missing {missing}; the T-STD-1 preimage is the full "
            f"{len(STD_HASH_COLUMNS)}-column projection of S8.3"
        )
    rendered = {column: _render(column, row[column]) for column in STD_HASH_COLUMNS}
    return content_hash(rendered, STD_HASH_COLUMNS)
