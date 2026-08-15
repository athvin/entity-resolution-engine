"""The expected-file layer of DesignDoc.md S8.2.1: loading, labels, sort key, digest.

``compare.py`` may expose exactly three functions and no others (S8.2.1), so
everything the comparison helpers need but that is not itself an assertion lives
here: the ``\\N`` null token, the absolute float tolerance, the byte-wise sort
key, the symbolic ``E1..En`` label space and a row digest.

Two rules are normative and are why this module exists rather than being inlined
into each caller:

* ``\\N`` is NULL and an empty field is the empty string; they are distinct
  values and an expected file never writes NULL as an empty field. Every column
  is read as VARCHAR, so no loader-side type inference can blur that distinction
  (or turn ``0.50`` into ``0.5``, or ``01`` into ``1``).
* ``entity_label`` is symbolic and MUST NEVER be a ULID. A fixture author who
  pastes a real ``entity_id`` into an expected file would produce a file that
  passes on the run it was captured from and fails on every other one, so the
  loader rejects a ULID-shaped label at read time rather than leaving that as a
  reviewer's duty.

:data:`VOLATILE_COLUMNS` is imported from ``src/er/lake/columns.py`` and never
re-listed here (S5.0); :func:`content_digest` reuses the S4.1 ``content_hash``
rather than re-deriving a second row encoding.
"""

from __future__ import annotations

import csv
import hashlib
import math
import re
from collections.abc import Iterable, Mapping, Sequence
from pathlib import Path

from er.ingest.hashing import UNIT_SEPARATOR, content_hash
from er.lake.columns import VOLATILE_COLUMNS

__all__ = [
    "FLOAT_TOL",
    "LABEL_COLUMN",
    "LABEL_PREFIX",
    "NULL_TOKEN",
    "Row",
    "as_float",
    "content_digest",
    "header_of",
    "label_map_from_membership",
    "load_expected",
    "sort_key",
]

#: One row of an expected file, every column read as VARCHAR, NULL as ``None``.
Row = dict[str, str | None]

#: S8.2.1: the null token is the two-character sequence backslash-N.
NULL_TOKEN = "\\N"

#: S8.2.1: `1e-9`, **absolute**, applied to `match_probability` and to numeric
#: golden columns. Every other column compares exactly.
FLOAT_TOL = 1e-9

#: S8.2.1's symbolic label space is `E1, E2, ... En`.
LABEL_PREFIX = "E"

#: The column an expected file carries a symbolic entity name in (S8.2.1).
LABEL_COLUMN = "entity_label"

#: A canonical ULID rendering: 26 characters of Crockford base32, which omits
#: `I`, `L`, `O` and `U`. Matched case-insensitively because a pasted lowercase
#: id is the same authoring mistake as a pasted uppercase one.
ULID_SHAPED = re.compile(r"[0-9A-HJKMNP-TV-Z]{26}", re.IGNORECASE)


def _records(path: Path) -> list[list[str]]:
    with path.open(encoding="utf-8", newline="") as handle:
        # A blank final line is how a text editor ends a file, not a row.
        return [record for record in csv.reader(handle) if record]


def header_of(path: Path) -> list[str]:
    """Return an expected file's column names, in file order.

    Separate from :func:`load_expected` because the header is a claim even when
    the file carries no rows: an ``expected/<phase>/events.csv`` asserting that a
    phase emits no events is a header and nothing else, and a comparison still
    has to know which columns it was asked about.
    """
    records = _records(path)
    if not records:
        raise ValueError(f"{path}: expected file has no header row")
    header = records[0]
    if len(set(header)) != len(header):
        raise ValueError(f"{path}: header repeats a column: {header}")
    return header


def load_expected(path: Path) -> list[Row]:
    """Read an S8.2.1 expected file, every column as VARCHAR.

    ``\\N`` becomes ``None`` and an empty field stays ``""``. Rows are returned
    in file order — comparison re-sorts both sides itself (:func:`sort_key`), so
    a mis-sorted committed file is a fixture-lint failure and never a scenario
    test failure — and each row's key order is the header order.

    Raises:
        ValueError: the file is empty, its header repeats a column, a data row's
            field count differs from the header's, or an ``entity_label`` value
            is missing or ULID-shaped.
    """
    records = _records(path)
    header = header_of(path)

    rows: list[Row] = []
    for line_number, record in enumerate(records[1:], start=2):
        if len(record) != len(header):
            raise ValueError(
                f"{path}:{line_number}: row has {len(record)} fields, header has {len(header)}"
            )
        row: Row = {
            column: (None if field == NULL_TOKEN else field)
            for column, field in zip(header, record, strict=True)
        }
        if LABEL_COLUMN in row:
            _check_label(row[LABEL_COLUMN], path, line_number)
        rows.append(row)
    return rows


def _check_label(label: str | None, path: Path, line_number: int) -> None:
    """Reject a missing or ULID-shaped ``entity_label`` (S8.2.1)."""
    if not label:
        raise ValueError(f"{path}:{line_number}: {LABEL_COLUMN} is empty or NULL")
    if ULID_SHAPED.fullmatch(label):
        raise ValueError(
            f"{path}:{line_number}: {LABEL_COLUMN} {label!r} is a ULID. Expected files carry "
            f"symbolic {LABEL_PREFIX}1..{LABEL_PREFIX}n labels, never a minted entity_id"
        )


def sort_key(row: Mapping[str, str | None], columns: Sequence[str]) -> tuple[bytes, ...]:
    """Return S8.2.1's sort key: the UTF-8 bytes of the column tuple, in header order.

    Per column rather than one concatenated string, so a value containing the
    delimiter cannot reorder rows; comparing a tuple of ``bytes`` compares byte
    values, which is the ordering the committed files are stored in and the one
    ``tests/unit/test_fixture_lint.py`` (ER-028) checks them against. NULL sorts
    as the two bytes of :data:`NULL_TOKEN` — what the file literally holds — so
    this key reproduces the file's own byte order.
    """
    return tuple(
        (NULL_TOKEN if row.get(column) is None else str(row.get(column))).encode("utf-8")
        for column in columns
    )


def label_map_from_membership(actual: Iterable[tuple[str, str]]) -> dict[str, str]:
    """Allocate the S8.2.1 label space over an actual partition.

    ``actual`` is ``(record_key, entity_id)`` pairs. Returns ``entity_label ->
    entity_id``, with ``E1..En`` allocated in ascending order of each group's
    minimum ``record_key`` — the same rule the expected files were authored
    under, which is what makes a captured map comparable across runs and across
    the two universes T-INC-1 builds. Mint order is deliberately not the rule:
    ULIDs are time-ordered and would make the map depend on the run.

    Raises:
        ValueError: a ``record_key`` appears under two ``entity_id``s. S5.0 has
            exactly one membership row per record, so that is a caller defect,
            not a partition this function may silently pick a winner for.
    """
    groups: dict[str, set[str]] = {}
    owner: dict[str, str] = {}
    for key, entity_id in actual:
        if key in owner and owner[key] != entity_id:
            raise ValueError(
                f"record_key {key!r} appears under two entity_ids: {owner[key]!r} and {entity_id!r}"
            )
        owner[key] = entity_id
        groups.setdefault(entity_id, set()).add(key)

    ordered = sorted(groups.items(), key=lambda item: min(item[1]))
    return {f"{LABEL_PREFIX}{index}": entity_id for index, (entity_id, _) in enumerate(ordered, 1)}


def content_digest(rows: Iterable[Mapping[str, str | None]], columns: Sequence[str]) -> str:
    """Digest ``rows`` over ``columns``: a row-order-independent determinism hash.

    The per-row digest is the S4.1 :func:`content_hash`, reused rather than
    re-implemented — S4.1 pins that encoding to exactly one function, and a
    second one here would disagree with it eventually. The row digests are
    sorted before they are folded, so two readings of the same relation in two
    orders agree; :data:`VOLATILE_COLUMNS` members are dropped from ``columns``
    because S5.0 excludes them from every determinism comparison, content hashes
    named first among them.

    In-memory rows only. The lake-side equivalent over a live relation is
    ``table_content_hash`` (ER-044).
    """
    stable = [column for column in columns if column not in VOLATILE_COLUMNS]
    digests = sorted(content_hash(row, stable) for row in rows)
    return hashlib.sha256(UNIT_SEPARATOR.join(digests).encode("utf-8")).hexdigest()


def as_float(value: str | None) -> float | None:
    """Parse ``value`` as a finite float, or return ``None`` if it is not one.

    ``nan`` and ``inf`` parse under :class:`float` but are not values a numeric
    comparison can be made about — ``nan != nan`` would report a cell as
    differing from itself — so they are treated as non-numeric and compared as
    the text they are.
    """
    if value is None:
        return None
    try:
        parsed = float(value)
    except ValueError:
        return None
    return parsed if math.isfinite(parsed) else None
