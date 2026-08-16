"""`base_10`'s committed expectations for the `base` phase (S8.2.1, S8.2, S5.0, S12).

Three files are committed here and only one of them is asserted against a live run
in this milestone: `std_hashes.csv` has `int_std_records` to compare against
(`tests/integration/scenarios/test_base_10_std.py`), while `membership.csv` and
`events.csv` describe `entity_membership` and `entity_events`, which M3 creates.
S12 authors all three in M2 anyway, so for two of them the *only* thing standing
between a mis-authored file and an M3 scenario failure four weeks from now is this
module. That is why it grades the files themselves rather than the pipeline.

What is graded, and why each is a fixture failure rather than a reviewer's duty:

* **the literal headers** -- read out of `EXPECTED_HEADERS` (ER-028), which is the
  single transcription of S8.2.1's header block. A header retyped here would be a
  second authority that could agree with the file and disagree with the spec.
* **the sort order** -- S8.2.1 stores every expected file ascending, byte-wise on
  the UTF-8 column tuple in header order, and says a mis-sorted committed file MUST
  be a lint failure and never a scenario-test failure. The negative arm below
  shuffles a copy of each file, because a sortedness check nobody has watched fail
  is indistinguishable from `assert True`.
* **the label rule** -- `E1..En` is allocated in ascending order of each group's
  minimum `record_key`, and the comparison helpers re-derive it at compare time.
  A hand-assigned label passes here only if it happens to equal what the rule
  produces, so the test re-derives it with the production helper
  (:func:`~helpers.expected.label_map_from_membership`) rather than restating the
  rule in a second implementation.
* **the truth grouping** -- `membership.csv` claims exactly the persona partition of
  `truth.csv`, so an edit to either that dissolves a persona fails here.

`std_hashes.csv` is deliberately NOT reproduced by this module: it can only come
from a real standardize run, and a unit test that recomputed it would be asserting
its own arithmetic. What is checkable without a lake is its shape, which is what
AC1 asks for; the value comparison is the integration arm's.
"""

from __future__ import annotations

import csv
import re
from collections.abc import Mapping, Sequence
from pathlib import Path

import pytest
from helpers.expected import (
    LABEL_COLUMN,
    LABEL_PREFIX,
    NULL_TOKEN,
    ULID_SHAPED,
    Row,
    header_of,
    label_map_from_membership,
    load_expected,
    sort_key,
)
from helpers.scenario import EXPECTED_HEADERS, expected_header_key, load_scenario

from er.entities.ids import record_key
from er.lake.columns import VOLATILE_COLUMNS
from er.lake.model import EVENT_TYPES

SCENARIO_NAME = "base_10"
PHASE = "base"

#: The three relations this milestone commits an expectation for. `golden.csv` and
#: `lineage` belong to M4, and an absent file is a phase making no claim (S8.2.1).
COMMITTED_RELATIONS: tuple[str, ...] = ("events", "membership", "std_hashes")

#: S8.2's ground-truth counts. Restated from the corpus rather than derived from
#: the expected files, so a file that lost rows cannot agree with itself.
EXPECTED_RECORDS = 23
EXPECTED_PERSONAS = 10
EXPECTED_PERSONA_SIZES = (4, 3, 3, 2, 2, 2, 2, 2, 2, 1)

#: T-STD-1's digest is a SHA-256 rendered as lowercase hex (S8.3), so 64 characters
#: of `[0-9a-f]` and nothing else -- an uppercase or truncated digest would compare
#: unequal to every computed one for a reason no diff would name.
STD_HASH_PATTERN = re.compile(r"[0-9a-f]{64}")

#: The only event a base phase over an empty lake can emit: every entity is new, so
#: there is nothing to add a member to, merge, split, retire or cut (S4.5).
BASE_PHASE_EVENT = "created"


@pytest.fixture(scope="module")
def expected_paths() -> Mapping[str, Path]:
    """The three committed files, resolved through ER-028's loader.

    Through the loader rather than by joining path fragments, because the loader is
    what reports "this phase makes no claim about that relation" as ``None`` -- and
    an expectation this ticket commits being absent must fail here rather than
    silently reduce the suite to two files.
    """
    scenario = load_scenario(SCENARIO_NAME)
    assert scenario.phases == (PHASE,)
    paths: dict[str, Path] = {}
    for relation in COMMITTED_RELATIONS:
        path = scenario.expected_path(PHASE, relation)
        assert path is not None, f"expected/{PHASE}/{relation}.csv is not committed"
        paths[relation] = path
    return paths


@pytest.fixture(scope="module")
def truth_groups() -> Mapping[str, frozenset[tuple[str, str]]]:
    """`truth.csv`'s persona partition, as `persona_id -> {(source_system, id)}`."""
    path = load_scenario(SCENARIO_NAME).truth["truth.csv"]
    with path.open(encoding="utf-8", newline="") as handle:
        rows = list(csv.DictReader(handle))
    grouped: dict[str, set[tuple[str, str]]] = {}
    for row in rows:
        grouped.setdefault(row["persona_id"], set()).add(
            (row["source_system"], row["source_record_id"])
        )
    assert len(rows) == EXPECTED_RECORDS
    return {persona: frozenset(members) for persona, members in grouped.items()}


def header_for(relation: str) -> tuple[str, ...]:
    """S8.2.1's literal header for one `expected/<phase>/` relation."""
    return EXPECTED_HEADERS[expected_header_key(relation)]


def out_of_order(rows: Sequence[Row], columns: Sequence[str]) -> list[int]:
    """The file line numbers whose sort key is below their predecessor's.

    Line numbers rather than a bare boolean: a mis-sorted file is an authoring
    mistake someone has to find, and "row 14 sorts before row 13" is the whole of
    the fix. `sort_key` is the helper the comparison layer uses, so this asks the
    same question S8.2.1 asks and not a similar one.
    """
    keys = [sort_key(row, columns) for row in rows]
    return [
        line
        for line, (previous, current) in enumerate(zip(keys, keys[1:], strict=False), start=3)
        if current < previous
    ]


def shuffled_copy(path: Path, destination: Path) -> Path:
    """`path` with its first two data rows swapped -- AC6's deliberate shuffle.

    A swap rather than a reversal: it is the smallest edit that breaks a strict
    ascending order, so a sortedness check that only notices wholesale reversal
    fails here too.
    """
    lines = path.read_text(encoding="utf-8").splitlines()
    assert len(lines) >= 3, f"{path}: needs two data rows to be shuffled"
    shuffled = [lines[0], lines[2], lines[1], *lines[3:]]
    destination.write_text("\n".join(shuffled) + "\n", encoding="utf-8")
    return destination


def membership_pairs(rows: Sequence[Row]) -> list[tuple[str, str]]:
    """`(record_key, entity_label)` per membership row, in file order.

    `record_key` comes from `ids.record_key` (S5.0) rather than from an f-string, so
    the key this test sorts labels by is the one the pipeline materializes.
    """
    pairs: list[tuple[str, str]] = []
    for row in rows:
        source_system = row["source_system"]
        source_record_id = row["source_record_id"]
        label = row[LABEL_COLUMN]
        assert source_system is not None and source_record_id is not None
        assert label is not None
        pairs.append((record_key(source_system, source_record_id), label))
    return pairs


# --------------------------------------------------------------------------- #
# AC6, and AC1/AC3/AC5's header clauses
# --------------------------------------------------------------------------- #


def test_headers_are_literal_and_sorted(expected_paths: Mapping[str, Path], tmp_path: Path) -> None:
    """Every committed file carries S8.2.1's header and is stored ascending."""
    for relation, path in sorted(expected_paths.items()):
        header = header_of(path)
        assert tuple(header) == header_for(relation), relation

        rows = load_expected(path)
        assert rows, f"{relation}: a committed expectation with no rows claims nothing"
        assert out_of_order(rows, header) == [], relation

    # The negative arm. Without it this test cannot distinguish a sorted corpus from
    # a sortedness check that never fires, and S8.2.1 puts the whole weight of
    # mis-sort detection on the lint rather than on the scenario tests.
    for relation, path in sorted(expected_paths.items()):
        shuffled = shuffled_copy(path, tmp_path / f"{relation}.csv")
        assert out_of_order(load_expected(shuffled), header_of(shuffled)) != [], (
            f"{relation}: swapping two rows produced no sort-order violation"
        )


def test_no_volatile_column_in_any_header(expected_paths: Mapping[str, Path]) -> None:
    """No expected file names a `VOLATILE_COLUMNS` member (S5.0, S8.2.1)."""
    # Non-vacuous: an empty exclusion set would make every header below pass.
    assert VOLATILE_COLUMNS

    for relation, path in sorted(expected_paths.items()):
        volatile = [column for column in header_of(path) if column in VOLATILE_COLUMNS]
        assert volatile == [], f"{relation}: {volatile} are run-scoped and never committed"


# --------------------------------------------------------------------------- #
# AC1
# --------------------------------------------------------------------------- #


def test_std_hashes_shape_and_hex(
    expected_paths: Mapping[str, Path], truth_groups: Mapping[str, frozenset[tuple[str, str]]]
) -> None:
    """23 rows, one per `base_10` record, each carrying a lowercase SHA-256."""
    path = expected_paths["std_hashes"]
    rows = load_expected(path)
    assert len(rows) == EXPECTED_RECORDS

    for line_number, row in enumerate(rows, start=2):
        # `load_expected` turns the `\N` token into None, so a NULL anywhere in this
        # file surfaces here: T-STD-1 hashes every current row, and a key with no
        # digest is a missing expectation rather than a null-valued one.
        assert None not in row.values(), f"{path}:{line_number}: {NULL_TOKEN} in a std_hash row"
        digest = row["std_hash"]
        assert digest is not None and STD_HASH_PATTERN.fullmatch(digest), (
            f"{path}:{line_number}: {digest!r} is not a lowercase 64-character SHA-256"
        )

    # The key set, against ground truth rather than against a count: 23 rows that
    # name 22 records and one twice would otherwise pass the length assertion.
    keys = [(row["source_system"], row["source_record_id"]) for row in rows]
    assert len(set(keys)) == EXPECTED_RECORDS
    assert set(keys) == {member for members in truth_groups.values() for member in members}


# --------------------------------------------------------------------------- #
# AC3
# --------------------------------------------------------------------------- #


def test_membership_groups_equal_truth_personas(
    expected_paths: Mapping[str, Path], truth_groups: Mapping[str, frozenset[tuple[str, str]]]
) -> None:
    """The committed grouping is `truth.csv`'s partition, relabelled `E1..E10`."""
    rows = load_expected(expected_paths["membership"])
    assert len(rows) == EXPECTED_RECORDS

    labels = {row[LABEL_COLUMN] for row in rows}
    assert labels == {f"{LABEL_PREFIX}{index}" for index in range(1, EXPECTED_PERSONAS + 1)}

    grouped: dict[str | None, set[tuple[str, str]]] = {}
    for row in rows:
        grouped.setdefault(row[LABEL_COLUMN], set()).add(
            (str(row["source_system"]), str(row["source_record_id"]))
        )

    # Set-equal as PARTITIONS: the label a group carries is this file's own naming
    # and truth.csv's `P<n>` is another, so comparing the sets of groups is the only
    # comparison that is about the grouping rather than about the two vocabularies.
    assert {frozenset(members) for members in grouped.values()} == set(truth_groups.values())
    assert tuple(sorted((len(members) for members in grouped.values()), reverse=True)) == (
        EXPECTED_PERSONA_SIZES
    )

    # The `persona_id` column is S8.2.1's crosswalk back to ground truth, so it must
    # agree with truth.csv row by row and not merely induce the same partition.
    for row in rows:
        member = (str(row["source_system"]), str(row["source_record_id"]))
        persona = row["persona_id"]
        assert persona is not None and member in truth_groups[persona], member


# --------------------------------------------------------------------------- #
# AC4
# --------------------------------------------------------------------------- #


def test_membership_labels_follow_min_record_key_rule(expected_paths: Mapping[str, Path]) -> None:
    """Re-deriving the labels by the S8.2.1 rule reproduces the committed ones."""
    path = expected_paths["membership"]
    rows = load_expected(path)

    # `label_map_from_membership` allocates `E1..En` over a partition by ascending
    # minimum `record_key`. Feeding it the committed labels in the slot a real
    # `entity_id` occupies makes it answer exactly AC4's question: the map is the
    # identity if and only if the file was labelled by the rule.
    derived = label_map_from_membership(membership_pairs(rows))
    mislabelled = {label: actual for label, actual in derived.items() if label != actual}
    assert mislabelled == {}, (
        f"{path}: labels are not allocated by ascending minimum record_key; "
        f"{mislabelled} should be relabelled left to right"
    )

    # AC4's second clause, over EVERY field rather than the label column alone: a
    # pasted `entity_id` is as wrong in `source_record_id` as in `entity_label`, and
    # the loader only guards the one column it knows carries a label.
    for line_number, row in enumerate(rows, start=2):
        for column, value in row.items():
            assert value is None or not ULID_SHAPED.fullmatch(value), (
                f"{path}:{line_number}: {column} is a ULID; expected files are ID-insensitive"
            )


# --------------------------------------------------------------------------- #
# AC5
# --------------------------------------------------------------------------- #


def test_events_is_ten_created_rows(expected_paths: Mapping[str, Path]) -> None:
    """One `created` row per label, count 1, and no other event type."""
    rows = load_expected(expected_paths["events"])
    membership_labels = {row[LABEL_COLUMN] for row in load_expected(expected_paths["membership"])}

    assert len(rows) == EXPECTED_PERSONAS
    assert {row[LABEL_COLUMN] for row in rows} == membership_labels
    assert len({row[LABEL_COLUMN] for row in rows}) == len(rows), "a label claims two event rows"

    for row in rows:
        assert row["event_type"] == BASE_PHASE_EVENT, row
        assert row["count"] == "1", row

    # The vocabulary is S5's, not this file's: an expectation naming an event the
    # `accepted_values` domain does not admit could never be produced by a run.
    assert BASE_PHASE_EVENT in EVENT_TYPES
