"""The three comparison helpers of DesignDoc.md S8.2.1 — and no others.

S8.2.1 pins this module's public surface at exactly
``assert_partition_equal``, ``assert_ids_stable`` and ``assert_golden_equal``.
The pinning is not stylistic: two comparison semantics are required because the
two guarantees are different. G3 compares two independently built universes
whose minted ULIDs legitimately differ, so only the induced set-partition may be
asserted; G2 asserts that identifiers *survive*, so the identifier itself is the
assertion and an ID-insensitive comparison would pass vacuously against a
pipeline that re-mints every entity on every run. A fourth helper here would be
a third semantics nobody chose. Everything else the three need lives in
``expected.py``.

Three rules hold for all three helpers:

* :data:`VOLATILE_COLUMNS` is *imported* from ``src/er/lake/columns.py`` and
  dropped from the expected side before anything is compared (S5.0). No
  exclusion list is written down here; there is exactly one, and it is there.
* Both sides are re-sorted by :func:`~helpers.expected.sort_key` before they are
  compared, so a mis-sorted committed file is a fixture-lint failure (ER-028)
  and never a scenario-test failure.
* A failure prints what diverged — the symmetric difference of the partitions,
  the offending ``(record_key, expected_id, actual_id)`` rows, or the differing
  cells — because these assertions fire inside integration runs that are
  expensive to reproduce.
"""

from __future__ import annotations

from collections.abc import Iterable, Mapping, Sequence
from pathlib import Path
from typing import Any

import pandas as pd

from er.entities.ids import record_key
from er.lake.columns import VOLATILE_COLUMNS

from .expected import (
    FLOAT_TOL,
    LABEL_COLUMN,
    Row,
    as_float,
    header_of,
    load_expected,
    sort_key,
)

__all__ = ["assert_golden_equal", "assert_ids_stable", "assert_partition_equal"]

#: The two S8.2.1 ``membership.csv`` columns a record key is assembled from.
_KEY_COLUMNS = ("source_system", "source_record_id")

#: ``golden_records``' key, and the column a golden frame is joined on after the
#: expected side's symbolic labels are resolved.
_ENTITY_ID = "entity_id"


def assert_partition_equal(actual: Iterable[tuple[str, str]], expected_csv: Path) -> None:
    """ID-INSENSITIVE. Groups record_keys by entity into frozensets and compares
    the set of frozensets. Used by T-INC-1.

    ``actual`` is ``(record_key, entity_id)`` pairs. The entity ids are read as
    grouping tokens only and are never compared, which is exactly the point:
    both universes T-INC-1 builds satisfy this against one committed expectation
    even though their ULIDs differ, and equality of both to that expectation is
    equality to each other.
    """
    expected_groups = _expected_groups(expected_csv)
    actual_groups = _actual_groups(actual)
    if expected_groups == actual_groups:
        return

    missing = sorted(expected_groups - actual_groups, key=sorted)
    unexpected = sorted(actual_groups - expected_groups, key=sorted)
    raise AssertionError(
        "\n".join(
            [
                f"partition differs from {expected_csv}",
                f"  groups expected but not produced ({len(missing)}):",
                *(f"    {sorted(group)}" for group in missing),
                f"  groups produced but not expected ({len(unexpected)}):",
                *(f"    {sorted(group)}" for group in unexpected),
            ]
        )
    )


def assert_ids_stable(
    actual: Iterable[tuple[str, str]],
    expected_csv: Path,
    label_map: dict[str, str],
) -> None:
    """ID-IDENTICAL. Resolves entity_label -> entity_id through a label_map
    captured earlier in the same test and asserts the exact entity_id per
    record. Used by T-PERM-1, T-PERM-2, T-PERM-3, T-MODEL-1.

    A record present on one side only is reported the same way as a record whose
    id changed, with ``None`` standing in for the side that lacks it.
    """
    rows, columns = _expected_rows(expected_csv)
    _require_columns(columns, (LABEL_COLUMN, *_KEY_COLUMNS), expected_csv)

    expected_ids = {
        _record_key_of(row): _resolve_label(row[LABEL_COLUMN], label_map, expected_csv)
        for row in rows
    }
    actual_ids = _actual_ids(actual)

    offending = [
        (key, expected_ids.get(key), actual_ids.get(key))
        for key in sorted(set(expected_ids) | set(actual_ids))
        if expected_ids.get(key) != actual_ids.get(key)
    ]
    if not offending:
        return
    raise AssertionError(
        "\n".join(
            [
                f"entity ids are not stable against {expected_csv}",
                "  (record_key, expected_id, actual_id):",
                *(f"    {row}" for row in offending),
            ]
        )
    )


def assert_golden_equal(
    actual: pd.DataFrame,
    expected_csv: Path,
    label_map: dict[str, str],
    float_tol: float = FLOAT_TOL,
) -> None:
    """Value comparison of golden_records after label resolution and volatile-
    column removal. Used by T-GOLD-1, T-INC-1, T-SNAP-1.

    The expected file's header is the claim: exactly its columns are compared,
    ``entity_label`` resolved to ``entity_id`` through ``label_map`` and any
    :data:`VOLATILE_COLUMNS` member dropped. A column whose every compared value
    parses as a finite float is compared within ``float_tol`` absolute; every
    other column compares exactly, both sides read as VARCHAR (S8.2.1).

    ``float_tol`` defaults to the S8.2.1 tolerance rather than re-spelling
    ``1e-9``: the number has one definition site, in ``expected.py``.
    """
    rows, columns = _expected_rows(expected_csv)
    _require_columns(columns, (LABEL_COLUMN,), expected_csv)
    value_columns = [column for column in columns if column != LABEL_COLUMN]

    expected_by_id: dict[str, Row] = {}
    for row in rows:
        entity_id = _resolve_label(row[LABEL_COLUMN], label_map, expected_csv)
        if entity_id in expected_by_id:
            raise AssertionError(f"{expected_csv}: two rows resolve to entity_id {entity_id!r}")
        expected_by_id[entity_id] = {
            _ENTITY_ID: entity_id,
            **{column: row[column] for column in value_columns},
        }

    actual_by_id = _frame_rows(actual, value_columns)
    _assert_same_entities(expected_by_id, actual_by_id, label_map, expected_csv)

    # Sorting is over `entity_id` first, which is unique on both sides, so the
    # two orderings agree however the committed file happened to be ordered.
    key_columns = [_ENTITY_ID, *value_columns]

    def in_sort_key_order(rows: Mapping[str, Row]) -> list[tuple[str, Row]]:
        return sorted(rows.items(), key=lambda item: sort_key(item[1], key_columns))

    ordered_expected = in_sort_key_order(expected_by_id)
    ordered_actual = in_sort_key_order(actual_by_id)

    numeric = {
        column
        for column in value_columns
        if _is_numeric_column(
            column,
            [row for _, row in ordered_expected],
            [row for _, row in ordered_actual],
        )
    }
    differences = [
        (entity_id, column, expected_row[column], actual_row[column])
        for (entity_id, expected_row), (_, actual_row) in zip(
            ordered_expected, ordered_actual, strict=True
        )
        for column in value_columns
        if not _cells_equal(expected_row[column], actual_row[column], column in numeric, float_tol)
    ]
    if not differences:
        return

    labels = {entity_id: label for label, entity_id in label_map.items()}
    raise AssertionError(
        "\n".join(
            [
                f"golden values differ from {expected_csv} (float_tol={float_tol})",
                "  (entity_label, column, expected, actual):",
                *(
                    f"    {(labels.get(entity_id, entity_id), column, expected, actual)}"
                    for entity_id, column, expected, actual in differences
                ),
            ]
        )
    )


def _expected_rows(expected_csv: Path) -> tuple[list[Row], list[str]]:
    """Load an expected file and drop every :data:`VOLATILE_COLUMNS` member.

    The single place the exclusion is applied, so all three helpers honour the
    imported set and none of them can grow a private list of its own (S5.0).
    """
    columns = [column for column in header_of(expected_csv) if column not in VOLATILE_COLUMNS]
    rows = [{column: row[column] for column in columns} for row in load_expected(expected_csv)]
    return rows, columns


def _require_columns(columns: Sequence[str], required: Sequence[str], expected_csv: Path) -> None:
    present = set(columns)
    missing = [column for column in required if column not in present]
    if missing:
        raise AssertionError(
            f"{expected_csv}: required column(s) {missing} absent after volatile columns were "
            f"dropped; comparable columns are {sorted(present)}"
        )


def _record_key_of(row: Row) -> str:
    system, record_id = (row[column] for column in _KEY_COLUMNS)
    if system is None or record_id is None:
        raise AssertionError(f"expected row has a NULL record key component: {row}")
    return record_key(system, record_id)


def _resolve_label(label: str | None, label_map: Mapping[str, str], expected_csv: Path) -> str:
    if label is None or label not in label_map:
        raise AssertionError(
            f"{expected_csv}: {LABEL_COLUMN} {label!r} is not in the captured label_map "
            f"{sorted(label_map)}"
        )
    return label_map[label]


def _expected_groups(expected_csv: Path) -> set[frozenset[str]]:
    rows, columns = _expected_rows(expected_csv)
    _require_columns(columns, (LABEL_COLUMN, *_KEY_COLUMNS), expected_csv)
    groups: dict[str, set[str]] = {}
    for row in rows:
        label = row[LABEL_COLUMN]
        if label is None:
            raise AssertionError(f"{expected_csv}: {LABEL_COLUMN} is NULL on row {row}")
        groups.setdefault(label, set()).add(_record_key_of(row))
    return {frozenset(members) for members in groups.values()}


def _actual_ids(actual: Iterable[tuple[str, str]]) -> dict[str, str]:
    ids: dict[str, str] = {}
    for key, entity_id in actual:
        if key in ids and ids[key] != entity_id:
            raise AssertionError(
                f"record_key {key!r} appears under two entity_ids: {ids[key]!r} and {entity_id!r}"
            )
        ids[key] = entity_id
    return ids


def _actual_groups(actual: Iterable[tuple[str, str]]) -> set[frozenset[str]]:
    groups: dict[str, set[str]] = {}
    for key, entity_id in _actual_ids(actual).items():
        groups.setdefault(entity_id, set()).add(key)
    return {frozenset(members) for members in groups.values()}


def _frame_rows(actual: pd.DataFrame, value_columns: Sequence[str]) -> dict[str, Row]:
    """Project a golden frame to ``entity_id`` plus ``value_columns``, as VARCHAR."""
    frame_columns = list(actual.columns)
    missing = [
        column for column in (_ENTITY_ID, *value_columns) if column not in set(frame_columns)
    ]
    if missing:
        raise AssertionError(
            f"golden frame is missing column(s) {missing}; it has {sorted(frame_columns)}"
        )

    rows: dict[str, Row] = {}
    records: list[Mapping[str, Any]] = actual.to_dict(orient="records")
    for record in records:
        entity_id = _cell(record[_ENTITY_ID])
        if entity_id is None:
            raise AssertionError(f"golden frame has a NULL {_ENTITY_ID}: {dict(record)}")
        if entity_id in rows:
            raise AssertionError(f"golden frame has two rows for {_ENTITY_ID} {entity_id!r}")
        rows[entity_id] = {
            _ENTITY_ID: entity_id,
            **{column: _cell(record[column]) for column in value_columns},
        }
    return rows


def _cell(value: Any) -> str | None:
    """Read one frame cell as VARCHAR. Every NULL spelling pandas has becomes ``None``."""
    if value is None or bool(pd.isna(value)):
        return None
    return str(value)


def _assert_same_entities(
    expected_by_id: Mapping[str, Row],
    actual_by_id: Mapping[str, Row],
    label_map: Mapping[str, str],
    expected_csv: Path,
) -> None:
    labels = {entity_id: label for label, entity_id in label_map.items()}
    missing = sorted(set(expected_by_id) - set(actual_by_id))
    unexpected = sorted(set(actual_by_id) - set(expected_by_id))
    if not missing and not unexpected:
        return
    raise AssertionError(
        "\n".join(
            [
                f"golden entities differ from {expected_csv}",
                f"  expected but absent: {[(labels.get(i, i), i) for i in missing]}",
                f"  present but unexpected: {unexpected}",
            ]
        )
    )


def _is_numeric_column(column: str, *sides: Sequence[Row]) -> bool:
    """A column is numeric when every value compared in it is a finite number.

    Decided per column rather than per cell, so S8.2.1's "any numeric golden
    column" reads as a property of the column: one non-numeric value anywhere in
    it — a name, a date, a formatted id — makes the whole column compare
    exactly, which is the stricter of the two readings.
    """
    values = [row[column] for side in sides for row in side if row[column] is not None]
    return bool(values) and all(as_float(value) is not None for value in values)


def _cells_equal(expected: str | None, actual: str | None, numeric: bool, float_tol: float) -> bool:
    if expected is None or actual is None:
        # `\N` and `""` are different values (S8.2.1), so a NULL is equal to a
        # NULL and to nothing else -- an empty string included.
        return expected is None and actual is None
    if numeric:
        left, right = as_float(expected), as_float(actual)
        if left is not None and right is not None:
            return abs(left - right) <= float_tol
    return expected == actual
