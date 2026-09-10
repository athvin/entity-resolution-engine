"""Fixture lint for the two split scenarios (S8.2, S8.2.1, S4.4, S4.5.3).

`split_scenario` exists to sever a previously merged entity with a `never`
assertion on a **bridge** edge, and `split_scenario_tie_2_2` to rank the even
fragments that severing leaves. The bridge property is a claim about a graph, so
it is DERIVED here from the committed CSVs rather than asserted in prose (AC1).

**The edge rule, and why it is sound.** An edge is designed between two records
iff they share an exact identifier -- a normalized email or a normalized phone.
That rule reproduces the committed model's `>= auto_merge` edge set on exactly
these fixtures because both directions are themselves machine-checked below:

* every email-sharing pair here also agrees on given name, family name,
  birth_date and postal (it is the same person), which clears `auto_merge` with
  bits to spare;
* every phone-sharing pair here also agrees on birth_date and postal, which is
  the designed bridge shape: `+12.68 + 12.28 + 9.55` bits of agreement against
  `given/family/email` disagreement leaves it comfortably above `auto_merge`
  under `model_test_v1`;
* every remaining pair shares **no S6 blocking key at all** -- not `email_exact`,
  not `phone_exact`, not `name_postal`, not `dob_name` -- so it is never a
  candidate, has no score, and cannot be an edge under any threshold.

The last point is what makes "adding any other edge makes the assertion fail"
(AC1) checkable: an edge can only be added by giving a cross pair a shared key,
and the no-shared-key assertion is exact. The pipeline-facing arbiter is
tests/integration/scenarios/test_split_scenario.py (T-PERM-2), which runs the
real model; this file proves the fixture's design, not the model's opinion.
"""

from __future__ import annotations

from collections.abc import Iterable, Mapping
from datetime import datetime
from pathlib import Path
from typing import Final

import pytest
from helpers.scenario import EXPECTED_HEADERS, load_scenario

from er.config.loader import load_config
from er.config.schema import Config
from er.review.assertions import ASSERTIONS_CSV_COLUMNS, NEVER

REPO_ROOT: Final = Path(__file__).resolve().parents[3]
FIXTURE_ROOT: Final = REPO_ROOT / "fixtures" / "static"
CONFIG_PATH: Final = REPO_ROOT / "configs" / "test.yaml"

SPLIT: Final = "split_scenario"
TIE: Final = "split_scenario_tie_2_2"
SCENARIOS: Final[tuple[str, ...]] = (SPLIT, TIE)

BASE: Final = "base"
BATCH: Final = "batch"

#: The fields the S6 comparisons read, in the spelling `sources.<name>.columns` maps.
COMPARED: Final = ("given_name", "family_name", "email", "phone", "birth_date", "addr_postal")

#: `name_postal` and `dob_name` as S6 writes them: a family-name prefix of four with
#: the postal, and the birth date with a given-name prefix of three.
FAMILY_PREFIX: Final = 4
GIVEN_PREFIX: Final = 3


def cfg() -> Config:
    return load_config(CONFIG_PATH)


def rows_of(path: Path) -> tuple[list[str], list[list[str]]]:
    """`(header, rows)` as raw split fields -- no null decoding, no re-sorting."""
    lines = path.read_text(encoding="utf-8").splitlines()
    assert lines, f"{path} is empty"
    return lines[0].split(","), [line.split(",") for line in lines[1:] if line]


def records(scenario: str, phase: str) -> dict[str, dict[str, str]]:
    """`record_key -> {logical field: comparable value}` for one phase's inputs.

    The normalization is the *comparable* part of S4.2 and no more, for the reason
    `tests/unit/fixtures/test_assertions_scenario.py` gives: the question asked here
    -- do these two records agree on this field? -- survives every normalization the
    real standardizer adds.
    """
    document = cfg()
    out: dict[str, dict[str, str]] = {}
    directory = FIXTURE_ROOT / scenario / phase
    for path in sorted(directory.glob("*.csv")):
        source = path.stem
        spec = document.sources[source]
        header, rows = rows_of(path)
        for row in rows:
            raw = dict(zip(header, row, strict=True))
            key = f"{source}:{raw[spec.record_id_column]}"
            fields: dict[str, str] = {}
            for logical in COMPARED:
                value = raw.get(spec.columns[logical], "").strip()
                if logical == "phone":
                    value = "".join(c for c in value if c.isdigit())
                elif logical == "birth_date" and value:
                    value = datetime.strptime(value, spec.date_format).date().isoformat()
                else:
                    value = value.lower()
                fields[logical] = value
            out[key] = fields
    return out


def personas(scenario: str, phase: str) -> dict[str, str]:
    """`record_key -> persona_id` from the delivered CSVs (S8.2's truth column)."""
    labels: dict[str, str] = {}
    directory = FIXTURE_ROOT / scenario / phase
    for path in sorted(directory.glob("*.csv")):
        header, rows = rows_of(path)
        spec = cfg().sources[path.stem]
        for row in rows:
            raw = dict(zip(header, row, strict=True))
            labels[f"{path.stem}:{raw[spec.record_id_column]}"] = raw["persona_id"]
    return labels


def shared_blocking_keys(left: Mapping[str, str], right: Mapping[str, str]) -> list[str]:
    """Which of the four S6 blocking rules pair these two records."""
    shared: list[str] = []
    if left["email"] and left["email"] == right["email"]:
        shared.append("email_exact")
    if left["phone"] and left["phone"] == right["phone"]:
        shared.append("phone_exact")
    if (
        left["family_name"]
        and right["family_name"]
        and left["addr_postal"]
        and (left["family_name"][:FAMILY_PREFIX], left["addr_postal"])
        == (right["family_name"][:FAMILY_PREFIX], right["addr_postal"])
    ):
        shared.append("name_postal")
    if (
        left["birth_date"]
        and right["birth_date"]
        and left["given_name"]
        and (left["birth_date"], left["given_name"][:GIVEN_PREFIX])
        == (right["birth_date"], right["given_name"][:GIVEN_PREFIX])
    ):
        shared.append("dob_name")
    return shared


def designed_edges(people: Mapping[str, dict[str, str]]) -> set[tuple[str, str]]:
    """Every canonical pair sharing an exact identifier -- the module's edge rule."""
    keys = sorted(people)
    return {
        (a, b)
        for index, a in enumerate(keys)
        for b in keys[index + 1 :]
        if (people[a]["email"] and people[a]["email"] == people[b]["email"])
        or (people[a]["phone"] and people[a]["phone"] == people[b]["phone"])
    }


def components(nodes: Iterable[str], edges: Iterable[tuple[str, str]]) -> list[frozenset[str]]:
    """Connected components, smallest-member order."""
    adjacency: dict[str, set[str]] = {node: set() for node in nodes}
    for a, b in edges:
        adjacency[a].add(b)
        adjacency[b].add(a)
    seen: set[str] = set()
    out: list[frozenset[str]] = []
    for node in sorted(adjacency):
        if node in seen:
            continue
        frontier, group = [node], {node}
        while frontier:
            for neighbour in adjacency[frontier.pop()]:
                if neighbour not in group:
                    group.add(neighbour)
                    frontier.append(neighbour)
        seen |= group
        out.append(frozenset(group))
    return sorted(out, key=min)


def asserted_pair(scenario: str) -> tuple[str, str, dict[str, str]]:
    """The scenario's single assertion row, header-checked."""
    header, rows = rows_of(FIXTURE_ROOT / scenario / "assertions.csv")
    assert tuple(header) == ASSERTIONS_CSV_COLUMNS, (
        f"{scenario}/assertions.csv header is {tuple(header)}; S8.2.1 pins {ASSERTIONS_CSV_COLUMNS}"
    )
    assert len(rows) == 1, f"{scenario} asserts {len(rows)} pairs; the design asserts one bridge"
    row = dict(zip(header, rows[0], strict=True))
    return row["rec_a_key"], row["rec_b_key"], row


# --------------------------------------------------------------------------- #
# AC1: the asserted pair is a bridge, machine-checked
# --------------------------------------------------------------------------- #


@pytest.mark.parametrize("scenario", SCENARIOS)
def test_asserted_pair_is_a_bridge(scenario: str) -> None:
    """Removing the asserted edge yields exactly two non-empty parts (AC1, AC7)."""
    people = records(scenario, BASE)
    labels = personas(scenario, BASE)
    rec_a, rec_b, _ = asserted_pair(scenario)
    assert {rec_a, rec_b} <= set(people), f"{scenario} asserts a pair it never delivered"

    # The soundness premises of the module's edge rule, checked rather than assumed.
    edges = designed_edges(people)
    for a, b in edges:
        if people[a]["email"] == people[b]["email"]:
            same = [
                f
                for f in ("given_name", "family_name", "birth_date", "addr_postal")
                if people[a][f] == people[b][f]
            ]
            assert len(same) == 4, (
                f"{scenario}: email-sharing pair ({a}, {b}) disagrees beyond phone; "
                "the edge rule's first premise no longer describes this fixture"
            )
        else:
            for field in ("birth_date", "addr_postal"):
                assert people[a][field] == people[b][field], (
                    f"{scenario}: phone-sharing pair ({a}, {b}) disagrees on {field}; "
                    "the bridge shape needs phone + birth_date + postal agreement"
                )

    # Non-edges are non-candidates: no shared blocking key, so no score under any
    # threshold. This is the exactness that makes AC1's "adding any other edge
    # makes the assertion fail" arm real.
    for a in sorted(people):
        for b in sorted(people):
            if a < b and (a, b) not in edges:
                assert not shared_blocking_keys(people[a], people[b]), (
                    f"{scenario}: ({a}, {b}) is not a designed edge but shares "
                    f"{shared_blocking_keys(people[a], people[b])}; it would be scored "
                    "and could become an undesigned second connection"
                )

    # The single base entity is the whole delivered record set (AC8's base half).
    membership_header, membership_rows = rows_of(
        FIXTURE_ROOT / scenario / "expected" / BASE / "membership.csv"
    )
    entity_of = {f"{system}:{record}": label for _, system, record, label in membership_rows}
    assert set(entity_of) == set(people)
    assert set(entity_of.values()) == {"E1"}, f"{scenario}'s base expectation is not one entity"

    whole = components(people, edges)
    assert whole == [frozenset(people)], f"{scenario}'s designed graph is not one component"

    assert (rec_a, rec_b) in edges, f"{scenario}'s asserted pair is not a designed edge"
    severed = components(people, edges - {(rec_a, rec_b)})
    assert len(severed) == 2 and all(severed), (
        f"{scenario}: removing ({rec_a}, {rec_b}) leaves {len(severed)} part(s); "
        "the asserted pair is not a bridge"
    )

    # The bridge is the persona boundary: each part is exactly one person's records,
    # so the steward's `never` is correcting a real false merge (S8.2).
    by_persona = {
        frozenset(key for key, persona in labels.items() if persona == name)
        for name in set(labels.values())
    }
    assert set(severed) == by_persona, (
        f"{scenario}: the severed parts {severed} are not the persona groups {by_persona}"
    )


def test_tie_scenario_fragments_are_even_and_ranked_by_min_record_key() -> None:
    """AC7's premise: two fragments of two, and the asserted survivor holds the minimum."""
    people = records(TIE, BASE)
    rec_a, rec_b, _ = asserted_pair(TIE)
    severed = components(people, designed_edges(people) - {(rec_a, rec_b)})
    sizes = sorted(len(part) for part in severed)
    assert sizes == [2, 2], f"the tie scenario severs into {sizes}, not 2-2"

    retaining, minted = sorted(severed, key=min)
    expected_header, expected_rows = rows_of(
        FIXTURE_ROOT / TIE / "expected" / BATCH / "membership.csv"
    )
    label_of = {f"{system}:{record}": label for _, system, record, label in expected_rows}
    assert {label_of[key] for key in retaining} == {"E1"}, (
        f"the fragment holding {min(retaining)} has the smaller minimum record_key and "
        "must keep the base entity's label E1 (S4.5.3 fragment ordering)"
    )
    assert {label_of[key] for key in minted} == {"E2"}, (
        "the other fragment must appear under a fresh label"
    )


# --------------------------------------------------------------------------- #
# AC2, AC3: the assertion row, and the batch exclusion
# --------------------------------------------------------------------------- #


@pytest.mark.parametrize("scenario", SCENARIOS)
def test_assertion_row_is_batch_never_and_canonical(scenario: str) -> None:
    rec_a, rec_b, row = asserted_pair(scenario)
    assert row["phase"] == BATCH, f"{scenario} asserts in phase {row['phase']!r}, not batch"
    assert row["kind"] == NEVER
    assert rec_a < rec_b, f"({rec_a!r}, {rec_b!r}) is not canonical (S5.0)"
    assert row["created_by"], "an assertion carries its steward"


@pytest.mark.parametrize("scenario", SCENARIOS)
def test_batch_contains_no_asserted_record(scenario: str) -> None:
    """AC3: the affected set is reached only through the assertion delta (S4.5.1)."""
    rec_a, rec_b, _ = asserted_pair(scenario)
    delivered = set(records(scenario, BATCH))
    assert delivered, f"{scenario}'s batch phase delivers nothing; there is no phase to run"
    assert not delivered & {rec_a, rec_b}, (
        f"{scenario}'s batch delivery contains asserted record(s) "
        f"{sorted(delivered & {rec_a, rec_b})}; the scenario then proves nothing about "
        "the assertion-delta arm of the affected node set"
    )


# --------------------------------------------------------------------------- #
# S8.2.1 format
# --------------------------------------------------------------------------- #


@pytest.mark.parametrize("scenario", SCENARIOS)
def test_expected_files_sorted_and_headers_literal(scenario: str) -> None:
    """Byte-sorted on the full column tuple, literal headers, symbolic labels."""
    load_scenario(scenario)  # the manifest parses and the layout is S8.2.1's

    root = FIXTURE_ROOT / scenario / "expected"
    committed = sorted(root.rglob("*.csv"))
    assert committed, f"{scenario} commits no expectation"
    for path in committed:
        key = f"expected/<phase>/{path.name}"
        header, rows = rows_of(path)
        assert tuple(header) == EXPECTED_HEADERS[key], (
            f"{path} header is {tuple(header)}; S8.2.1 pins {EXPECTED_HEADERS[key]}"
        )
        assert rows == sorted(rows), f"{path} is not byte-sorted on its column tuple"
        if path.name == "membership.csv":
            for row in rows:
                assert row[-1].startswith("E") and row[-1][1:].isdigit(), (
                    f"{path}: {row[-1]!r} is not a symbolic entity label (S8.2.1)"
                )

    for scenario_dir in (SPLIT, TIE):
        golden = list((FIXTURE_ROOT / scenario_dir / "expected").rglob("golden.csv"))
        assert not golden, f"{golden} committed before M4 (S12 gating)"
