"""Unit tests for the S8.2.1 comparison helpers and the expected-file layer.

Nine of the eleven scenario tests rest on the distinction these tests exist to
pin: `assert_partition_equal` is ID-INSENSITIVE and `assert_ids_stable` is
ID-IDENTICAL, and the two are not interchangeable. So the central pair of tests
feed **the same** re-minted partition to both helpers and require one to accept
it and the other to reject it — an ID-insensitive `assert_ids_stable` would pass
vacuously against a pipeline that re-mints every entity on every run (G2), and
an ID-sensitive `assert_partition_equal` could never compare T-INC-1's two
independently built universes (G3).

The rest assert the encoding rules S8.2.1 states as text: `\\N` is NULL and an
empty field is not, the float tolerance is 1e-9 absolute and applies to numeric
columns only, both sides are re-sorted byte-wise before comparison, the label
space is `E1..En` in ascending minimum-`record_key` order and never a ULID, and
the volatile-column exclusion is *imported* from `src/er/lake/columns.py` — the
last of which is checked twice, once behaviourally and once against the source
text, because a re-listed copy would pass the behavioural half on the day it was
written and drift silently afterwards.
"""

from __future__ import annotations

import ast
import inspect
from collections.abc import Iterable, Sequence
from pathlib import Path

import pandas as pd
import pytest
from helpers import compare
from helpers.compare import assert_golden_equal, assert_ids_stable, assert_partition_equal
from helpers.expected import (
    FLOAT_TOL,
    NULL_TOKEN,
    content_digest,
    label_map_from_membership,
    load_expected,
    sort_key,
)

from er.entities.ids import CountingIdFactory, UlidFactory, record_key
from er.lake.columns import VOLATILE_COLUMNS

REPO_ROOT = Path(__file__).resolve().parents[2]
HELPERS_DIR = REPO_ROOT / "tests" / "helpers"

# S8.2.1: `compare.py` exposes exactly these three functions and no others.
S8_2_1_PUBLIC = {"assert_partition_equal", "assert_ids_stable", "assert_golden_equal"}

MEMBERSHIP_HEADER = ("persona_id", "source_system", "source_record_id", "entity_label")
MEMBERSHIP_ROWS = (
    ("P1", "billing", "b1", "E1"),
    ("P1", "crm", "c1", "E1"),
    ("P2", "crm", "c2", "E2"),
    ("P2", "webforms", "w2", "E2"),
)

GOLDEN_HEADER = (
    "entity_label",
    "given_name",
    "family_name",
    "email",
    "addr_number",
    "survivorship_version",
)
GOLDEN_ROWS = (
    ("E1", "Ada", "Lovelace", "ada@example.com", "10", "v1"),
    ("E2", "Grace", "Hopper", NULL_TOKEN, "11", "v1"),
)

# One numeric column and one that is not, so the tolerance test can show the rule
# applying to the first and not to the second.
SCORE_HEADER = ("entity_label", "given_name", "match_probability")
SCORE_ROWS = (("E1", "Ada", "0.5"),)


def write_csv(path: Path, header: Sequence[str], rows: Iterable[Sequence[str]]) -> Path:
    path.write_text(
        "\n".join([",".join(header), *(",".join(row) for row in rows)]) + "\n", encoding="utf-8"
    )
    return path


def membership_csv(tmp_path: Path, rows: Sequence[Sequence[str]] = MEMBERSHIP_ROWS) -> Path:
    return write_csv(tmp_path / "membership.csv", MEMBERSHIP_HEADER, rows)


def counted_partition() -> tuple[list[tuple[str, str]], dict[str, str]]:
    """The partition `MEMBERSHIP_ROWS` describes, under deterministic ids.

    Returns `(record_key, entity_id)` pairs and the label map they induce.
    """
    factory = CountingIdFactory(start=1)
    first, second = factory.new(), factory.new()
    actual = [
        (record_key("billing", "b1"), first),
        (record_key("crm", "c1"), first),
        (record_key("crm", "c2"), second),
        (record_key("webforms", "w2"), second),
    ]
    return actual, {"E1": first, "E2": second}


def remint(actual: Sequence[tuple[str, str]]) -> list[tuple[str, str]]:
    """The same grouping under freshly minted ULIDs — G3's legitimate difference."""
    factory = UlidFactory()
    fresh = {entity_id: factory.new() for _, entity_id in actual}
    return [(key, fresh[entity_id]) for key, entity_id in actual]


def golden_frame(
    label_map: dict[str, str],
    rows: Sequence[Sequence[str]] = GOLDEN_ROWS,
    **overrides: str,
) -> pd.DataFrame:
    """A `golden_records` frame matching `rows`, with labels resolved to ids."""
    records = []
    for row in rows:
        values = dict(zip(GOLDEN_HEADER, row, strict=True))
        record: dict[str, str | None] = {"entity_id": label_map[values.pop("entity_label")]}
        for column, value in values.items():
            record[column] = None if value == NULL_TOKEN else value
        record.update(overrides)
        # A VOLATILE_COLUMNS member on the actual side. It is never compared,
        # because the expected header never names one.
        record["assembled_at"] = "2026-01-01T00:00:00Z"
        records.append(record)
    return pd.DataFrame.from_records(records)


def with_value(
    rows: Sequence[Sequence[str]], index: int, column: str, value: str
) -> tuple[tuple[str, ...], ...]:
    """`rows` with one cell replaced, addressed by column name."""
    position = GOLDEN_HEADER.index(column)
    return tuple(
        tuple(value if i == position else cell for i, cell in enumerate(row))
        if row_index == index
        else tuple(row)
        for row_index, row in enumerate(rows)
    )


def string_constants(path: Path) -> set[str]:
    """Every string literal in `path` that is not a docstring."""
    tree = ast.parse(path.read_text(encoding="utf-8"))
    docstrings = {
        ast.get_docstring(node, clean=False)
        for node in ast.walk(tree)
        if isinstance(node, ast.Module | ast.FunctionDef | ast.AsyncFunctionDef | ast.ClassDef)
    }
    literals = {
        node.value
        for node in ast.walk(tree)
        if isinstance(node, ast.Constant) and isinstance(node.value, str)
    }
    return literals - docstrings


def test_compare_module_exposes_exactly_three_functions() -> None:
    """AC1: the public surface is the three S8.2.1 functions, with their pinned signatures."""
    public = {
        name
        for name, value in vars(compare).items()
        if not name.startswith("_")
        and callable(value)
        and getattr(value, "__module__", None) == compare.__name__
    }
    assert public == S8_2_1_PUBLIC, "a fourth public function in compare.py violates S8.2.1"
    assert set(compare.__all__) == S8_2_1_PUBLIC

    assert list(inspect.signature(assert_partition_equal).parameters) == ["actual", "expected_csv"]
    assert list(inspect.signature(assert_ids_stable).parameters) == [
        "actual",
        "expected_csv",
        "label_map",
    ]
    signature = inspect.signature(assert_golden_equal)
    assert list(signature.parameters) == ["actual", "expected_csv", "label_map", "float_tol"]
    assert signature.parameters["float_tol"].default == FLOAT_TOL == 1e-9


def test_partition_equal_is_id_insensitive(tmp_path: Path) -> None:
    """AC2: re-minted ids pass; one record moving between groups fails, printing both sides."""
    expected_csv = membership_csv(tmp_path)
    actual, _ = counted_partition()
    assert_partition_equal(actual, expected_csv)
    assert_partition_equal(remint(actual), expected_csv)

    reminted = remint(actual)
    other = reminted[-1][1]
    moved = [
        (key, other if key == record_key("crm", "c1") else entity_id) for key, entity_id in reminted
    ]
    with pytest.raises(AssertionError) as excinfo:
        assert_partition_equal(moved, expected_csv)

    message = str(excinfo.value)
    assert "expected but not produced" in message
    assert "produced but not expected" in message
    assert record_key("crm", "c1") in message
    assert record_key("billing", "b1") in message


def test_ids_stable_is_id_identical(tmp_path: Path) -> None:
    """AC3: the input AC2 accepts is rejected here — the two helpers are not interchangeable."""
    expected_csv = membership_csv(tmp_path)
    actual, label_map = counted_partition()
    assert label_map_from_membership(actual) == label_map
    assert_ids_stable(actual, expected_csv, label_map)

    reminted = remint(actual)
    assert_partition_equal(reminted, expected_csv)
    with pytest.raises(AssertionError) as excinfo:
        assert_ids_stable(reminted, expected_csv, label_map)

    message = str(excinfo.value)
    assert "(record_key, expected_id, actual_id)" in message
    assert label_map["E1"] in message and label_map["E2"] in message
    assert record_key("billing", "b1") in message

    # A record the run never produced is reported the same way, as `None`.
    with pytest.raises(AssertionError, match="None"):
        assert_ids_stable(actual[1:], expected_csv, label_map)


def test_volatile_columns_are_imported_not_relisted(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """AC4: all three helpers drop the imported set, and no helper re-lists it."""
    expected_csv = write_csv(tmp_path / "golden.csv", GOLDEN_HEADER, GOLDEN_ROWS)
    actual, label_map = counted_partition()
    frame = golden_frame(label_map, given_name="Adah")

    with pytest.raises(AssertionError, match="given_name"):
        assert_golden_equal(frame, expected_csv, label_map)

    # Adding a member to the imported set stops the column being compared, which
    # it could not do if the helper carried its own list.
    monkeypatch.setattr(compare, "VOLATILE_COLUMNS", VOLATILE_COLUMNS | {"given_name"})
    assert_golden_equal(frame, expected_csv, label_map)

    # The other two helpers read the same set: dropping `entity_label` leaves
    # them with no label column at all, and they say so.
    monkeypatch.setattr(compare, "VOLATILE_COLUMNS", VOLATILE_COLUMNS | {"entity_label"})
    membership = membership_csv(tmp_path)
    with pytest.raises(AssertionError, match="entity_label"):
        assert_partition_equal(actual, membership)
    with pytest.raises(AssertionError, match="entity_label"):
        assert_ids_stable(actual, membership, label_map)

    sources = sorted(HELPERS_DIR.glob("*.py"))
    assert sources, "tests/helpers must hold the helper modules"
    for source in sources:
        relisted = VOLATILE_COLUMNS & string_constants(source)
        assert not relisted, f"{source} re-lists {sorted(relisted)} instead of importing the set"


def test_null_token_distinct_from_empty_string(tmp_path: Path) -> None:
    """AC5: `\\N` loads as None, an empty field as "", and the two never compare equal."""
    assert NULL_TOKEN == "\\N"
    null_csv = write_csv(tmp_path / "null.csv", GOLDEN_HEADER, GOLDEN_ROWS)
    empty_rows = with_value(GOLDEN_ROWS, 1, "email", "")
    empty_csv = write_csv(tmp_path / "empty.csv", GOLDEN_HEADER, empty_rows)

    assert load_expected(null_csv)[1]["email"] is None
    assert load_expected(empty_csv)[1]["email"] == ""

    _, label_map = counted_partition()
    null_frame = golden_frame(label_map)
    empty_frame = golden_frame(label_map, rows=empty_rows)
    assert_golden_equal(null_frame, null_csv, label_map)
    assert_golden_equal(empty_frame, empty_csv, label_map)

    with pytest.raises(AssertionError, match="email"):
        assert_golden_equal(empty_frame, null_csv, label_map)
    with pytest.raises(AssertionError, match="email"):
        assert_golden_equal(null_frame, empty_csv, label_map)


def test_float_tolerance_boundary(tmp_path: Path) -> None:
    """AC6: 9e-10 passes and 1.1e-9 fails on a numeric column; text differs at any distance."""
    expected_csv = write_csv(tmp_path / "golden.csv", SCORE_HEADER, SCORE_ROWS)
    _, label_map = counted_partition()

    def frame(probability: str, given_name: str = "Ada") -> pd.DataFrame:
        return pd.DataFrame.from_records(
            [
                {
                    "entity_id": label_map["E1"],
                    "given_name": given_name,
                    "match_probability": probability,
                }
            ]
        )

    assert_golden_equal(frame("0.5000000009"), expected_csv, label_map)
    with pytest.raises(AssertionError, match="match_probability"):
        assert_golden_equal(frame("0.5000000011"), expected_csv, label_map)

    # A non-numeric column is exact: the tolerance is not a general fuzziness.
    with pytest.raises(AssertionError, match="given_name"):
        assert_golden_equal(frame("0.5", given_name="Adah"), expected_csv, label_map)

    # Absolute, and the caller may tighten it.
    with pytest.raises(AssertionError, match="match_probability"):
        assert_golden_equal(frame("0.5000000009"), expected_csv, label_map, float_tol=1e-12)


def test_both_sides_are_resorted_before_comparison(tmp_path: Path) -> None:
    """AC7: a shuffled expected file compares equal to the same content sorted."""
    sorted_csv = write_csv(tmp_path / "sorted.csv", GOLDEN_HEADER, GOLDEN_ROWS)
    shuffled_csv = write_csv(tmp_path / "shuffled.csv", GOLDEN_HEADER, tuple(reversed(GOLDEN_ROWS)))
    actual, label_map = counted_partition()

    frame = golden_frame(label_map, rows=tuple(reversed(GOLDEN_ROWS)))
    assert_golden_equal(frame, sorted_csv, label_map)
    assert_golden_equal(frame, shuffled_csv, label_map)

    shuffled_membership = write_csv(
        tmp_path / "membership.csv", MEMBERSHIP_HEADER, tuple(reversed(MEMBERSHIP_ROWS))
    )
    assert_partition_equal(actual, shuffled_membership)
    assert_ids_stable(actual, shuffled_membership, label_map)

    # Byte-wise, so 'Z' (0x5a) precedes 'a' (0x61): neither locale collation nor
    # case folding may reorder a committed file.
    rows = [{"value": "a"}, {"value": "Z"}]
    assert [row["value"] for row in sorted(rows, key=lambda row: sort_key(row, ["value"]))] == [
        "Z",
        "a",
    ]
    # NULL sorts as the two bytes the file literally holds.
    assert sort_key({"value": None}, ["value"]) == (NULL_TOKEN.encode("utf-8"),)


def test_label_allocation_order_and_ulid_rejection(tmp_path: Path) -> None:
    """AC8: E1..En follow the minimum record_key, not mint order; a ULID label is refused."""
    factory = CountingIdFactory(start=1)
    first, second = factory.new(), factory.new()
    # The entity minted FIRST holds the lexically LATER minimum record_key, so
    # mint order and label order disagree and only one of them can be the rule.
    actual = [
        (record_key("webforms", "w2"), first),
        (record_key("crm", "c2"), first),
        (record_key("billing", "b1"), second),
        (record_key("crm", "c1"), second),
    ]
    label_map = label_map_from_membership(actual)
    assert label_map == {"E1": second, "E2": first}
    assert_ids_stable(actual, membership_csv(tmp_path), label_map)

    assert len(second) == 26, "the rejection below is only meaningful against a real ULID"
    pasted = tuple(
        (persona, system, record_id, second if label == "E1" else label)
        for persona, system, record_id, label in MEMBERSHIP_ROWS
    )
    ulid_csv = write_csv(tmp_path / "ulid.csv", MEMBERSHIP_HEADER, pasted)
    with pytest.raises(ValueError, match="is a ULID"):
        load_expected(ulid_csv)


def test_content_digest_is_row_order_stable(tmp_path: Path) -> None:
    """AC4/M3: the digest folds sorted row hashes and ignores volatile columns."""
    rows = load_expected(membership_csv(tmp_path))
    columns = list(MEMBERSHIP_HEADER)
    digest = content_digest(rows, columns)

    assert content_digest(list(reversed(rows)), columns) == digest
    assert content_digest(rows[:-1], columns) != digest

    # A volatile column in the projection cannot move it (S5.0).
    volatile = sorted(VOLATILE_COLUMNS)[0]
    stamped = [{**row, volatile: "2026-01-01T00:00:00Z"} for row in rows]
    assert content_digest(stamped, [*columns, volatile]) == digest

    changed = [{**rows[0], "entity_label": "E9"}, *rows[1:]]
    assert content_digest(changed, columns) != digest
