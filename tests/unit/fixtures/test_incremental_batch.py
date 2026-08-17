"""The `incremental_batch` scenario and the structure it was authored to have (S8.2, S8.2.1).

S8.2 asks this fixture for six new records — three joining existing entities, two
forming a new entity, one bridging two existing entities and forcing a merge —
because the "two form a new entity" case is reachable only through the new-vs-new
Splink pass (S4.3.4, D2). Nothing downstream notices if that shape quietly
dissolves: drop the new-vs-new pass and every assertion in M3 still passes, because
no record would have needed it. These six tests are what turn each structural claim
back into a fixture failure, from the committed files alone.

Three of them are worth stating as decisions rather than as assertions:

* **`base/` is committed, not inherited.** S3 shows this scenario with its own
  `base/` and S8.2.1 makes a missing phase directory mean the scenario has no such
  phase, so `base_scenario: base_10` is a byte-identity assertion rather than an
  indirection — and the assertion is made here, over the bytes, which is what keeps
  the two corpora from drifting apart one edit at a time.
* **The labels are recomputed, never inherited.** S8.2.1 allocates `E1..En` per
  expected file in ascending order of the group's minimum `record_key`, so the batch
  phase's labels are NOT base_10's: the merge and the new entity move six of the ten.
  The rule is re-derived with the production helper
  (:func:`~helpers.expected.label_map_from_membership`) rather than restated, so a
  hand-assigned label passes only if it is what the rule produces.
* **The bridge is cross-persona by construction and says so.** base_10's ten
  personas are exactly ten entities (S8.2), so no truthful bridge exists in it. The
  merged pair is declared in `scenario.yaml` (`bridged_labels`, `bridged_personas`)
  and cross-checked here against base_10's own `expected/base/membership.csv`, so the
  designed mechanic cannot be mistaken for a quality defect.

`expected/batch/std_hashes.csv` is not reproduced here — it can only come from a real
`er ingest` + `dbt build`, and a unit test that recomputed it would be asserting its
own arithmetic. What is checkable without a lake is that its 23 base rows are the
digests `base_10` already committed, which is AC7's regression arm.
"""

from __future__ import annotations

import csv
import re
from collections.abc import Mapping, Sequence
from pathlib import Path

import pytest
from helpers.expected import (
    LABEL_COLUMN,
    NULL_TOKEN,
    ULID_SHAPED,
    Row,
    header_of,
    label_map_from_membership,
    load_expected,
    sort_key,
)
from helpers.scenario import (
    BRIDGE_ARITY,
    EXPECTED_HEADERS,
    Scenario,
    expected_header_key,
    load_scenario,
)

from er.config.loader import load_config
from er.config.schema import CANONICAL_ATTRIBUTES, Config, SourceSpec
from er.entities.ids import record_key
from er.lake.columns import VOLATILE_COLUMNS
from er.lake.model import EVENT_TYPES

REPO_ROOT = Path(__file__).resolve().parents[3]
TEST_CONFIG_PATH = REPO_ROOT / "configs" / "test.yaml"

SCENARIO_NAME = "incremental_batch"
BASE_SCENARIO_NAME = "base_10"
PHASE = "batch"

#: The truth column S8.2 puts last on every input row. It never reaches the
#: pipeline, which is why it is named here and in no `sources.columns` mapping.
PERSONA_COLUMN = "persona_id"

#: The three relations this ticket commits an expectation for. `golden.csv` belongs
#: to M4 and an absent expected file is a phase making no claim (S8.2.1, S12).
COMMITTED_RELATIONS: tuple[str, ...] = ("events", "membership", "std_hashes")

#: S8.2's corpus, and this scenario's delivery on top of it.
BASE_RECORDS = 23
BATCH_RECORDS = 6
TOTAL_RECORDS = BASE_RECORDS + BATCH_RECORDS

#: base_10's ten entities, minus the one the bridge absorbs, plus the one the
#: new-vs-new pair mints: the batch phase ends on ten entities again, and the
#: arithmetic is spelled out because the coincidence is the thing that hides a
#: fixture that lost its bridge or its new pair.
BASE_ENTITIES = 10
BATCH_ENTITIES = BASE_ENTITIES - 1 + 1

#: S8.2.1's `attribution.csv` columns and its two vocabularies.
#: S8.2.1's three roles. `new_pair` is spelled `PASS_2_ROLE` because that is what it
#: is for: it is the only role the new-vs-new pass can discover, every other role
#: having a partner already in the corpus (S4.3.4, D2).
ATTRIBUTION_HEADER: tuple[str, ...] = ("record_key", "role", "pass")
JOINER_ROLE, BRIDGE_ROLE, PASS_2_ROLE = "joiner", "bridge", "new_pair"
ROLE_COUNTS: Mapping[str, int] = {JOINER_ROLE: 3, BRIDGE_ROLE: 1, PASS_2_ROLE: 2}
PASS_1, PASS_2 = "pass1", "pass2"

#: The three event types the batch phase emits (S4.5.3): the claimant absorbs the
#: entity the bridge joined it to, the new-vs-new pair mints one, and each record
#: that joined an existing entity is added to it.
MERGED_EVENT, CREATED_EVENT, MEMBER_ADDED_EVENT = "merged", "created", "member_added"

#: T-STD-1's digest is a SHA-256 rendered as lowercase hex (S8.3).
STD_HASH_PATTERN = re.compile(r"[0-9a-f]{64}")

#: Spellings of NULL that are not S8.2.1's `\N`. An empty field is deliberately
#: absent: S8.2.1 makes the empty string a distinct value from NULL.
ALTERNATE_NULLS = frozenset({"NULL", "Null", "null", "None", "NaN", "nan"})


# --------------------------------------------------------------------------- #
# fixtures
# --------------------------------------------------------------------------- #


@pytest.fixture(scope="module")
def scenario() -> Scenario:
    """The scenario, opened through ER-028's loader."""
    loaded = load_scenario(SCENARIO_NAME)
    assert loaded.phases == ("base", PHASE)
    return loaded


@pytest.fixture(scope="module")
def base_scenario() -> Scenario:
    """`base_10`, whose deliveries and labels this scenario is defined against."""
    return load_scenario(BASE_SCENARIO_NAME)


@pytest.fixture(scope="module")
def config() -> Config:
    """The S6 document S8.2's headers are derived from."""
    return load_config(TEST_CONFIG_PATH)


@pytest.fixture(scope="module")
def expected_paths(scenario: Scenario) -> Mapping[str, Path]:
    """The three committed `expected/batch/` files, through the loader.

    Through the loader rather than by joining path fragments, because the loader is
    what reports "this phase makes no claim about that relation" as ``None`` -- and
    an expectation this ticket commits being absent must fail here rather than
    silently reduce the suite to two files.
    """
    paths: dict[str, Path] = {}
    for relation in COMMITTED_RELATIONS:
        path = scenario.expected_path(PHASE, relation)
        assert path is not None, f"expected/{PHASE}/{relation}.csv is not committed"
        paths[relation] = path
    return paths


@pytest.fixture(scope="module")
def membership_rows(expected_paths: Mapping[str, Path]) -> list[Row]:
    return load_expected(expected_paths["membership"])


@pytest.fixture(scope="module")
def attribution(scenario: Scenario) -> list[dict[str, str]]:
    """`attribution.csv`, S8.2.1 ground truth read only by tests."""
    path = scenario.truth.get("attribution.csv")
    assert path is not None, "attribution.csv is not committed at the scenario root"
    with path.open(encoding="utf-8", newline="") as handle:
        reader = csv.DictReader(handle)
        assert tuple(reader.fieldnames or ()) == ATTRIBUTION_HEADER
        return list(reader)


# --------------------------------------------------------------------------- #
# helpers
# --------------------------------------------------------------------------- #


def header_for(relation: str) -> tuple[str, ...]:
    """S8.2.1's literal header for one `expected/<phase>/` relation."""
    return EXPECTED_HEADERS[expected_header_key(relation)]


def derived_header(spec: SourceSpec) -> str:
    """S8.2's input header for one source, derived from S6 and from nothing else.

    `record_id_column`, then the `columns` values in canonical-attribute order,
    then `updated_at_column`, then the truth column.
    """
    fields = (
        spec.record_id_column,
        *(spec.columns[attribute] for attribute in CANONICAL_ATTRIBUTES),
        spec.updated_at_column,
        PERSONA_COLUMN,
    )
    return ",".join(fields)


def read_input(path: Path) -> list[dict[str, str]]:
    with path.open(encoding="utf-8", newline="") as handle:
        return list(csv.DictReader(handle))


def refs(rows: Sequence[Row]) -> list[tuple[str, str]]:
    """The `(source_system, source_record_id)` of each membership row."""
    return [(str(row["source_system"]), str(row["source_record_id"])) for row in rows]


def groups_by_label(rows: Sequence[Row]) -> dict[str, frozenset[tuple[str, str]]]:
    """`entity_label -> the set of records it holds`."""
    grouped: dict[str, set[tuple[str, str]]] = {}
    for row in rows:
        label = row[LABEL_COLUMN]
        assert label is not None
        grouped.setdefault(label, set()).add(
            (str(row["source_system"]), str(row["source_record_id"]))
        )
    return {label: frozenset(members) for label, members in grouped.items()}


def label_of(rows: Sequence[Row], ref: tuple[str, str]) -> str:
    """The label the membership file gives one record."""
    for row in rows:
        if (row["source_system"], row["source_record_id"]) == ref:
            label = row[LABEL_COLUMN]
            assert label is not None
            return label
    raise AssertionError(f"{ref} has no membership row")


def split_key(key: str) -> tuple[str, str]:
    """`source_system:source_record_id` back into the pair the expected files carry."""
    source_system, separator, source_record_id = key.partition(":")
    assert separator, f"{key!r} is not a record_key"
    return source_system, source_record_id


def out_of_order(rows: Sequence[Row], columns: Sequence[str]) -> list[int]:
    """The file line numbers whose sort key is below their predecessor's."""
    keys = [sort_key(row, columns) for row in rows]
    return [
        line
        for line, (previous, current) in enumerate(zip(keys, keys[1:], strict=False), start=3)
        if current < previous
    ]


def shuffled_copy(path: Path, destination: Path) -> Path:
    """`path` with its first two data rows swapped: the sortedness check's failing arm."""
    lines = path.read_text(encoding="utf-8").splitlines()
    assert len(lines) >= 3, f"{path}: needs two data rows to be shuffled"
    destination.write_text("\n".join([lines[0], lines[2], lines[1], *lines[3:]]) + "\n", "utf-8")
    return destination


# --------------------------------------------------------------------------- #
# AC1, and AC7
# --------------------------------------------------------------------------- #


def test_base_is_byte_identical_to_base_10(
    scenario: Scenario, base_scenario: Scenario, expected_paths: Mapping[str, Path]
) -> None:
    """`base/` and the 23 base `std_hash` rows are base_10's, byte for byte."""
    assert scenario.manifest.base_scenario == BASE_SCENARIO_NAME
    assert scenario.base_chain == (BASE_SCENARIO_NAME,)

    ours = scenario.inputs_for("base")
    theirs = base_scenario.inputs_for("base")
    assert set(ours) == set(theirs)
    # Committed here rather than inherited: `_phase_inputs` walks the base_scenario
    # chain, so a `base/` this scenario had lost would silently resolve to base_10's
    # and this test would compare a file with itself.
    for source, path in sorted(ours.items()):
        assert path.parent.parent == scenario.root, f"{source}: base/ is not committed here"
        assert path.read_bytes() == theirs[source].read_bytes(), source

    base_hashes = base_scenario.expected_path("base", "std_hashes")
    assert base_hashes is not None
    committed = expected_paths["std_hashes"].read_text(encoding="utf-8").splitlines()
    inherited = base_hashes.read_text(encoding="utf-8").splitlines()

    assert len(committed) == TOTAL_RECORDS + 1, "29 digests plus the header"
    assert len(inherited) == BASE_RECORDS + 1
    # Line-for-line rather than as a set: the base rows are the same rows in the same
    # byte order, and a digest that moved must name itself in the diff.
    assert set(inherited[1:]) <= set(committed[1:]), (
        "a base_10 digest is missing from expected/batch/std_hashes.csv; the base "
        "delivery is byte-identical, so every one of its 23 rows must reappear here"
    )

    for row in load_expected(expected_paths["std_hashes"]):
        digest = row["std_hash"]
        assert digest is not None and STD_HASH_PATTERN.fullmatch(digest), row


# --------------------------------------------------------------------------- #
# AC2
# --------------------------------------------------------------------------- #


def test_batch_shape_and_no_key_collisions(scenario: Scenario, config: Config) -> None:
    """Six rows over the three sources, S8.2's headers, and no key already in base."""
    batch = scenario.inputs_for(PHASE)
    assert set(batch) == set(config.sources)

    base_keys = {
        (source, row[config.sources[source].record_id_column])
        for source, path in scenario.inputs_for("base").items()
        for row in read_input(path)
    }
    assert len(base_keys) == BASE_RECORDS

    batch_keys: set[tuple[str, str]] = set()
    delivered = 0
    for source, path in sorted(batch.items()):
        spec = config.sources[source]
        assert path.read_text(encoding="utf-8").splitlines()[0] == derived_header(spec), source
        for row in read_input(path):
            assert row[PERSONA_COLUMN], f"{source}: a batch row carries no {PERSONA_COLUMN}"
            batch_keys.add((source, row[spec.record_id_column]))
            delivered += 1

    assert delivered == BATCH_RECORDS
    assert len(batch_keys) == BATCH_RECORDS
    # A collision would make the delivery a supersession rather than an insertion
    # (S4.2's greatest-`ingested_at` rule), which is a different scenario entirely.
    assert batch_keys.isdisjoint(base_keys), sorted(batch_keys & base_keys)


# --------------------------------------------------------------------------- #
# AC3, and AC4
# --------------------------------------------------------------------------- #


def test_attribution_roles_and_passes(
    scenario: Scenario,
    base_scenario: Scenario,
    config: Config,
    attribution: list[dict[str, str]],
) -> None:
    """Six records, one role each, and pass 2 exactly where the new-vs-new pair is."""
    keyed = {row["record_key"]: row for row in attribution}
    assert len(keyed) == len(attribution) == BATCH_RECORDS

    delivered = {
        record_key(source, row[config.sources[source].record_id_column])
        for source, path in scenario.inputs_for(PHASE).items()
        for row in read_input(path)
    }
    assert set(keyed) == delivered

    roles: dict[str, list[str]] = {}
    for key, row in sorted(keyed.items()):
        roles.setdefault(row["role"], []).append(key)
    assert {role: len(keys) for role, keys in roles.items()} == dict(ROLE_COUNTS)

    # The whole point of the file: pass 2 is claimed by the new-vs-new pair and by
    # nothing else, so a run that omitted that pass has a named, checkable gap.
    assert {key for key, row in keyed.items() if row["pass"] == PASS_2} == set(roles[PASS_2_ROLE])
    assert {row["pass"] for row in attribution} == {PASS_1, PASS_2}
    for key, row in sorted(keyed.items()):
        expected_pass = PASS_2 if row["role"] == PASS_2_ROLE else PASS_1
        assert row["pass"] == expected_pass, key

    # AC4: the declared bridge names two labels that are two distinct groups of two
    # distinct personas in base_10's own membership file.
    labels = scenario.manifest.bridged_labels
    personas = scenario.manifest.bridged_personas
    assert len(labels) == len(personas) == BRIDGE_ARITY

    base_membership = base_scenario.expected_path("base", "membership")
    assert base_membership is not None
    base_rows = load_expected(base_membership)
    base_groups = groups_by_label(base_rows)
    assert set(labels) <= set(base_groups), (labels, sorted(base_groups))

    left, right = (base_groups[label] for label in labels)
    assert left and right and left.isdisjoint(right)
    persona_of = {
        (str(row["source_system"]), str(row["source_record_id"])): str(row[PERSONA_COLUMN])
        for row in base_rows
    }
    assert [sorted({persona_of[ref] for ref in group}) for group in (left, right)] == [
        [personas[0]],
        [personas[1]],
    ]
    assert personas[0] != personas[1], "a bridge that merges one persona is not cross-persona"


# --------------------------------------------------------------------------- #
# AC5
# --------------------------------------------------------------------------- #


def test_expected_batch_membership_partition(
    scenario: Scenario,
    base_scenario: Scenario,
    membership_rows: list[Row],
    attribution: list[dict[str, str]],
) -> None:
    """29 rows, ten labels, the bridged union group and the new-vs-new pair's group."""
    assert len(membership_rows) == TOTAL_RECORDS
    assert len(set(refs(membership_rows))) == TOTAL_RECORDS

    groups = groups_by_label(membership_rows)
    assert len(groups) == BATCH_ENTITIES

    # The labels are this file's own allocation and MUST NOT be assumed equal to
    # base_10's: re-derive them by S8.2.1's rule with the production helper, which
    # is the identity map exactly when the file was labelled by that rule.
    labelled = [
        (record_key(*ref), str(row[LABEL_COLUMN]))
        for ref, row in zip(refs(membership_rows), membership_rows, strict=True)
    ]
    derived = label_map_from_membership(labelled)
    mislabelled = {label: actual for label, actual in derived.items() if label != actual}
    assert mislabelled == {}, (
        f"labels are not allocated by ascending minimum record_key; {mislabelled} "
        "should be relabelled left to right"
    )

    roles = {row["record_key"]: row["role"] for row in attribution}
    (bridge_key,) = [key for key, role in roles.items() if role == BRIDGE_ROLE]
    new_pair = {split_key(key) for key, role in roles.items() if role == PASS_2_ROLE}

    base_membership = base_scenario.expected_path("base", "membership")
    assert base_membership is not None
    base_groups = groups_by_label(load_expected(base_membership))
    bridged = frozenset().union(*(base_groups[label] for label in scenario.manifest.bridged_labels))

    surviving = label_of(membership_rows, split_key(bridge_key))
    assert groups[surviving] == bridged | {split_key(bridge_key)}

    (new_label,) = {label_of(membership_rows, ref) for ref in new_pair}
    assert groups[new_label] == new_pair
    assert new_label != surviving


# --------------------------------------------------------------------------- #
# AC6
# --------------------------------------------------------------------------- #


def test_expected_batch_events_counts(
    expected_paths: Mapping[str, Path],
    membership_rows: list[Row],
    attribution: list[dict[str, str]],
) -> None:
    """One `merged`, one `created`, and four `member_added` on the right labels."""
    rows = load_expected(expected_paths["events"])
    labels = set(groups_by_label(membership_rows))

    for row in rows:
        # The vocabulary is S5's, not this file's: an expectation naming an event the
        # `accepted_values` domain does not admit could never be produced by a run.
        assert row["event_type"] in EVENT_TYPES, row
        assert row[LABEL_COLUMN] in labels, row
    assert len({(row[LABEL_COLUMN], row["event_type"]) for row in rows}) == len(rows)

    by_type: dict[str, dict[str, int]] = {}
    for row in rows:
        count = row["count"]
        assert count is not None and count.isdigit(), row
        by_type.setdefault(str(row["event_type"]), {})[str(row[LABEL_COLUMN])] = int(count)
    assert set(by_type) == {MERGED_EVENT, CREATED_EVENT, MEMBER_ADDED_EVENT}

    roles = {row["record_key"]: row["role"] for row in attribution}
    (bridge_key,) = [key for key, role in roles.items() if role == BRIDGE_ROLE]
    surviving = label_of(membership_rows, split_key(bridge_key))
    new_pair = {split_key(key) for key, role in roles.items() if role == PASS_2_ROLE}
    (new_label,) = {label_of(membership_rows, ref) for ref in new_pair}

    # The absorbed entity holds no member after the merge, so it has no symbolic
    # label and cannot be named: the surviving label carries the `merged` row.
    assert by_type[MERGED_EVENT] == {surviving: 1}
    assert by_type[CREATED_EVENT] == {new_label: 1}

    # Every record that joined an existing entity, and no other: the four records the
    # attribution assigns to pass 1, each landing on the label it joined.
    joined = [
        label_of(membership_rows, split_key(key))
        for key, role in roles.items()
        if role != PASS_2_ROLE
    ]
    assert by_type[MEMBER_ADDED_EVENT] == {label: joined.count(label) for label in joined}
    assert sum(by_type[MEMBER_ADDED_EVENT].values()) == len(joined) == BATCH_RECORDS - 2


# --------------------------------------------------------------------------- #
# AC8
# --------------------------------------------------------------------------- #


def test_expected_files_encoding_and_sort(
    expected_paths: Mapping[str, Path], tmp_path: Path
) -> None:
    """Literal headers, byte-ascending storage, no volatile column, no ULID, no NULL."""
    # Non-vacuous: an empty exclusion set would make every header below pass.
    assert VOLATILE_COLUMNS

    for relation, path in sorted(expected_paths.items()):
        header = header_of(path)
        assert tuple(header) == header_for(relation), relation
        assert [column for column in header if column in VOLATILE_COLUMNS] == [], relation

        rows = load_expected(path)
        assert rows, f"{relation}: a committed expectation with no rows claims nothing"
        assert out_of_order(rows, header) == [], relation

        for line_number, row in enumerate(rows, start=2):
            for column, value in row.items():
                where = f"{path}:{line_number}: {column}"
                # `load_expected` maps `\N` to None, so a None here is a NULL the
                # file really wrote -- and none of these three relations has one.
                assert value is not None, f"{where} is {NULL_TOKEN}"
                assert value not in ALTERNATE_NULLS, f"{where} spells NULL as {value!r}"
                assert not ULID_SHAPED.fullmatch(value), f"{where} is a ULID"

    # The negative arm. Without it this test cannot distinguish a sorted corpus from
    # a sortedness check that never fires, and S8.2.1 puts the whole weight of
    # mis-sort detection on the lint rather than on the scenario tests.
    for relation, path in sorted(expected_paths.items()):
        shuffled = shuffled_copy(path, tmp_path / f"{relation}.csv")
        assert out_of_order(load_expected(shuffled), header_of(shuffled)) != [], relation
