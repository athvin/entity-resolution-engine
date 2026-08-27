"""Fixture lint for the two merge scenarios (S8.2.1), over the committed bytes.

None of this needs a lake, and that is the point: a committed expectation is a claim
about a partition, and a claim whose *encoding* is wrong fails a scenario test with a
diff that looks like a pipeline defect. These arms fail with a diff that looks like what
it is.

Three properties are worth stating separately from "the scenario test passes":

* **Sorted, byte-wise, on the UTF-8 column tuple.** `load_expected` deliberately returns
  rows in file order and re-sorts both sides before comparing (S8.2.1), so a mis-sorted
  committed file never fails a scenario test — it just makes the file harder to read and
  its diffs larger than the change that caused them. Only a lint catches it.
* **`entity_label` is symbolic.** M7: a committed ULID would pin an expectation to the
  run that produced it, and every later run would fail against it for the one reason
  that carries no information.
* **The tie scenario's premise holds in the committed data.** AC6 asks for exactly equal
  overlap. If a later edit gave one entity a third member, the tiebreak would stop being
  what decides the survivor and `test_claimant_tiebreak_selects_min_record_key_survivor`
  would still pass — for the wrong reason. This asserts the premise directly.
"""

from __future__ import annotations

from pathlib import Path
from typing import Final

import pytest
from helpers.scenario import EXPECTED_HEADERS, load_scenario

REPO_ROOT: Final = Path(__file__).resolve().parents[3]
FIXTURE_ROOT: Final = REPO_ROOT / "fixtures" / "static"

MERGE_SCENARIO: Final = "merge_scenario"
TIE_SCENARIO: Final = "merge_scenario_tie"
SCENARIOS: Final[tuple[str, ...]] = (MERGE_SCENARIO, TIE_SCENARIO)

BASE_PHASE: Final = "base"
BATCH_PHASE: Final = "batch"
PHASES: Final[tuple[str, ...]] = (BASE_PHASE, BATCH_PHASE)

#: S8.2.1's null token. The only one admitted, so an empty field is an empty string and
#: never a null, and the two are never spelled the same way.
NULL_TOKEN: Final = "\\N"

MEMBERSHIP: Final = "membership.csv"
EVENTS: Final = "events.csv"
RELATIONS: Final[tuple[str, ...]] = (MEMBERSHIP, EVENTS)

MERGED: Final = "merged"

#: The tie scenario's two old entities, as `record_key` tuples. Spelled here so the
#: equal-overlap assertion is a claim this file makes rather than one it reads off the
#: very data it is checking.
TIE_FIRST: Final[tuple[str, ...]] = ("billing:B601", "crm:C601")
TIE_SECOND: Final[tuple[str, ...]] = ("billing:B602", "webforms:W602")


def expected_path(scenario: str, phase: str, relation: str) -> Path:
    return FIXTURE_ROOT / scenario / "expected" / phase / relation


def rows_of(path: Path) -> tuple[list[str], list[list[str]]]:
    """`(header, rows)` as raw split fields — no null decoding, no re-sorting."""
    lines = path.read_text(encoding="utf-8").splitlines()
    assert lines, f"{path} is empty"
    return lines[0].split(","), [line.split(",") for line in lines[1:] if line]


def membership_of(scenario: str, phase: str) -> dict[str, str]:
    """`record_key -> entity_label` from a committed membership expectation."""
    _, rows = rows_of(expected_path(scenario, phase, MEMBERSHIP))
    return {f"{system}:{record}": label for _, system, record, label in rows}


@pytest.mark.parametrize("scenario", SCENARIOS)
@pytest.mark.parametrize("phase", PHASES)
@pytest.mark.parametrize("relation", RELATIONS)
def test_expected_files_are_sorted_and_headers_literal(
    scenario: str, phase: str, relation: str
) -> None:
    """AC1: the committed order is the sorted order, and the header is S8.2.1's literal."""
    path = expected_path(scenario, phase, relation)
    header, rows = rows_of(path)

    key = f"expected/<phase>/{relation}"
    assert tuple(header) == EXPECTED_HEADERS[key], (
        f"{path} header is {tuple(header)}; S8.2.1 pins {EXPECTED_HEADERS[key]}"
    )
    assert rows, f"{path} carries a header and no rows; it claims nothing"
    assert rows == sorted(rows), (
        f"{path} is not sorted on its UTF-8 column tuple. `load_expected` re-sorts both "
        f"sides before comparing, so this never fails a scenario test — it only makes "
        f"the file's diffs larger than the change that caused them.\n"
        f"  committed: {rows}\n  sorted:    {sorted(rows)}"
    )

    for row in rows:
        for field in row:
            assert field == field.strip(), f"{path} carries a padded field: {row}"
            assert field not in {"NULL", "null", "None", "NA", "\\n"}, (
                f"{path} spells a null as {field!r}; S8.2.1's only null token is {NULL_TOKEN!r}"
            )


@pytest.mark.parametrize("scenario", SCENARIOS)
@pytest.mark.parametrize("phase", PHASES)
def test_entity_labels_are_symbolic(scenario: str, phase: str) -> None:
    """M7: `entity_label` is `E1`, `E2`, … and never a ULID."""
    labels = set(membership_of(scenario, phase).values())
    assert labels, f"{scenario}/{phase} membership carries no labels"
    for label in labels:
        assert label.startswith("E") and label[1:].isdigit(), (
            f"{scenario}/{phase} carries entity_label {label!r}. A committed ULID pins "
            "the expectation to the run that minted it, so every later run fails "
            "against it for the one reason that carries no information (M7)."
        )


@pytest.mark.parametrize("scenario", SCENARIOS)
def test_base_and_batch_labels_encode_a_merge(scenario: str) -> None:
    """AC2: two labels before, one after, and exactly one `merged` event."""
    base = membership_of(scenario, BASE_PHASE)
    batch = membership_of(scenario, BATCH_PHASE)

    assert len(set(base.values())) == 2, (
        f"{scenario} base holds {sorted(set(base.values()))}; a merge needs exactly two "
        "entities to start from"
    )
    assert len(set(batch.values())) == 1, (
        f"{scenario} batch holds {sorted(set(batch.values()))}; after the merge every "
        "record belongs to one entity"
    )
    assert set(base) < set(batch), (
        f"{scenario}'s batch phase does not add a record to the base set; the bridge is "
        "what causes the merge and it must appear in the batch expectation"
    )

    _, events = rows_of(expected_path(scenario, BATCH_PHASE, EVENTS))
    merged = [row for row in events if row[1] == MERGED]
    assert len(merged) == 1 and merged[0][2] == "1", (
        f"{scenario} batch events carry {merged}; S4.5.3 emits exactly one `merged` "
        "event for one merge"
    )

    # The event is on the LOSER — the entity that lost all its members — so its label
    # must be one that the base phase used and the batch phase no longer does.
    survivor = next(iter(set(batch.values())))
    assert merged[0][0] != survivor, (
        f"{scenario}'s merged event is on {merged[0][0]}, the surviving entity. S4.5.3 "
        "merges the old entity INTO the claimant, so the event belongs to the one that "
        "was merged away."
    )
    assert merged[0][0] in set(base.values()), (
        f"{scenario}'s merged event names {merged[0][0]}, which the base phase never "
        "held; the loser must be one of the two entities that existed before"
    )


def test_tie_scenario_overlaps_are_equal() -> None:
    """AC6: equal overlap, so only `min member record_key ASC` can pick the survivor."""
    base = membership_of(TIE_SCENARIO, BASE_PHASE)
    assert set(base) == {*TIE_FIRST, *TIE_SECOND}, (
        f"the tie scenario's base membership is {sorted(base)}; this file's premise "
        f"names {sorted((*TIE_FIRST, *TIE_SECOND))}"
    )

    first = {base[key] for key in TIE_FIRST}
    second = {base[key] for key in TIE_SECOND}
    assert len(first) == 1 and len(second) == 1 and first != second, (
        f"the tie scenario's base phase is not two whole entities: {base}"
    )
    assert len(TIE_FIRST) == len(TIE_SECOND), (
        f"the two old entities hold {len(TIE_FIRST)} and {len(TIE_SECOND)} members. "
        "With unequal sizes the largest-overlap half of S4.5.3's claimant rule decides "
        "the survivor and the tiebreak is never reached, so the scenario would pass "
        "without testing what it exists to test."
    )

    survivor_label = next(iter(set(membership_of(TIE_SCENARIO, BATCH_PHASE).values())))
    assert survivor_label == first.pop(), (
        f"the tie scenario's committed survivor is {survivor_label!r}, but "
        f"{min(TIE_FIRST)!r} is the smaller minimum member record_key of two equal "
        f"overlaps, so the entity holding it is the one S4.5.3 keeps. Swapping the "
        "committed expectation is what this assertion refuses."
    )


@pytest.mark.parametrize("scenario", SCENARIOS)
@pytest.mark.parametrize("phase", PHASES)
def test_no_golden_expectation_is_committed(scenario: str, phase: str) -> None:
    """AC7: `golden.csv` is absent, which is how S8.2.1 spells the M4 deferral."""
    golden = expected_path(scenario, phase, "golden.csv")
    assert not golden.exists(), (
        f"{golden} exists. `golden_records` is first written in M4 and reaping the "
        "loser's golden row is T-INC-2's assertion (ER-092), so a golden expectation "
        "here is a claim this milestone cannot check. S8.2.1 makes an absent file mean "
        "the phase claims nothing about that relation."
    )


@pytest.mark.parametrize("scenario", SCENARIOS)
def test_manifest_declares_both_phases(scenario: str) -> None:
    """The manifest and the committed expectation directories agree (S8.2.1)."""
    loaded = load_scenario(scenario)
    assert loaded.phases == PHASES, f"{scenario} declares phases {loaded.phases}"
