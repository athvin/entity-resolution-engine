"""Fixture lint for the six assertion scenarios (S8.2, S8.2.1, S4.4).

Every designed property of these fixtures is DERIVED here from the committed CSVs
rather than asserted in prose, because a fixture's design is a claim and prose cannot
fail. Three of those derivations carry the weight:

* **The cut tie is a computed tie, not a hoped-for one.** A tie in `match_probability`
  cannot be checked by a test that does not score. What *can* be checked is the input to
  the score: :func:`agreement_pattern` maps each record's raw columns through
  `sources.<name>.columns` and reports, field by field, whether the pair agrees, differs
  or is null on both sides. Two edges with identical patterns have identical
  probabilities **by INV-SCORE** (S4.3.3), which makes the tie a consequence of the spec
  rather than an assumption about Splink.

* **The path and iteration counts come from the designed edge set.** The same
  agreement-pattern function decides which pairs are edges, so the hop counts this file
  asserts are counted over the graph the fixture actually encodes — not over the one its
  author meant to encode.

* **An absent claim is checked as an absent file.** `assertions_contradiction` fails
  CONTRADICTION-1 in its batch phase, so there is no post-state to describe. S8.2.1
  spells that as an absent file, and the test asserts the absence rather than trusting a
  comment to explain it.

The committed expectations were generated from real runs of all six scenarios and then
compared, never authored. Running them through the pipeline as scenario tests is ER-079.
"""

from __future__ import annotations

import subprocess
import sys
from collections.abc import Iterable
from datetime import datetime
from pathlib import Path
from typing import Final

import pytest
from helpers.scenario import EXPECTED_HEADERS, load_scenario

from er.config.loader import load_config
from er.config.schema import Config
from er.review.assertions import ALWAYS, ASSERTIONS_CSV_COLUMNS, NEVER, PHASES

REPO_ROOT: Final = Path(__file__).resolve().parents[3]
FIXTURE_ROOT: Final = REPO_ROOT / "fixtures" / "static"
CONFIG_PATH: Final = REPO_ROOT / "configs" / "test.yaml"

MAIN: Final = "assertions_scenario"
CONTRADICTION: Final = "assertions_contradiction"
NON_BATCH: Final = "assertions_non_batch"
CUT_TIE: Final = "assertions_cut_tie"
PATH_TIE: Final = "assertions_path_tie"
TWO_ITERATIONS: Final = "assertions_two_iterations"

SCENARIOS: Final[tuple[str, ...]] = (
    MAIN,
    CONTRADICTION,
    NON_BATCH,
    CUT_TIE,
    PATH_TIE,
    TWO_ITERATIONS,
)

BASE: Final = "base"
BATCH: Final = "batch"

EDGE_CUT: Final = "edge_cut"
NULL_TOKEN: Final = "\\N"

#: The four cases A1-A4, by the canonical pair each is asserted over.
A1: Final = ("billing:B701", "crm:C701")
A2: Final = ("billing:B702", "crm:C702")
A3: Final = ("billing:B703", "crm:C703")
A4: Final = ("billing:B704", "crm:C704")

#: A3's third record: the one that keeps the never pair connected.
A3_THIRD: Final = "webforms:W703"

#: The comparison columns S6 declares, which are the only fields a probability depends
#: on. `address_line`, `addr_city` and `addr_region` are standardized but not compared,
#: so they are excluded here — including them would make two edges look different when
#: the model cannot tell them apart.
COMPARED: Final[tuple[str, ...]] = (
    "given_name",
    "family_name",
    "email",
    "phone",
    "addr_postal",
    "birth_date",
)

AGREE: Final = "agree"
DIFFER: Final = "differ"
ABSENT: Final = "absent"


def cfg() -> Config:
    """The S6 document these fixtures are designed against."""
    return load_config(CONFIG_PATH)


def rows_of(path: Path) -> tuple[list[str], list[list[str]]]:
    """`(header, rows)` as raw split fields — no null decoding, no re-sorting."""
    lines = path.read_text(encoding="utf-8").splitlines()
    assert lines, f"{path} is empty"
    return lines[0].split(","), [line.split(",") for line in lines[1:] if line]


def expected(scenario: str, phase: str, relation: str) -> Path:
    return FIXTURE_ROOT / scenario / "expected" / phase / relation


def records(scenario: str, phase: str) -> dict[str, dict[str, str]]:
    """`record_key -> {logical field: normalized value}` for one phase's inputs.

    The normalization is deliberately the *comparable* part of S4.2 and no more: case and
    padding for text, digits only for a phone, and the source's own `date_format` for a
    birth date. A fixture lint that reimplemented the full standardizer would be a second
    implementation of it, and the properties asserted here — do these two records agree
    on this field? — survive every normalization the real one adds.
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


def agreement_pattern(left: dict[str, str], right: dict[str, str]) -> tuple[tuple[str, str], ...]:
    """Per-field `agree` / `differ` / `absent` for one pair (S4.3.3's INV-SCORE key).

    `absent` is its own outcome rather than a kind of agreement, because every comparison
    in the committed model carries `is_null_level` and Splink scores a null level at a
    Bayes factor of 1 — no evidence either way. Folding it into `agree` would make two
    edges look identical when one carries real evidence and the other carries none.
    """
    pattern = []
    for field in COMPARED:
        a, b = left[field], right[field]
        if not a or not b:
            pattern.append((field, ABSENT))
        else:
            pattern.append((field, AGREE if a == b else DIFFER))
    return tuple(pattern)


def links(left: dict[str, str], right: dict[str, str]) -> bool:
    """Whether the fixture designs an edge between two records.

    True when no compared field DIFFERS. That is the design rule every scenario here is
    built on, and it is sound under the committed model for the reason the module
    docstring gives: a disagreement on `phone_e164` alone contributes a Bayes factor of
    1.63e-18 and annihilates the product, while an absent field contributes 1.
    """
    return all(outcome != DIFFER for _, outcome in agreement_pattern(left, right))


def designed_edges(scenario: str, phase: str = BASE) -> set[tuple[str, str]]:
    """Every canonical pair the fixture designs an edge between."""
    people = records(scenario, phase)
    keys = sorted(people)
    return {
        (a, b)
        for index, a in enumerate(keys)
        for b in keys[index + 1 :]
        if links(people[a], people[b])
    }


def assertion_rows(scenario: str) -> list[dict[str, str]]:
    """`assertions.csv` as dicts, in file order."""
    header, rows = rows_of(FIXTURE_ROOT / scenario / "assertions.csv")
    assert tuple(header) == ASSERTIONS_CSV_COLUMNS, (
        f"{scenario}/assertions.csv header is {tuple(header)}; S8.2.1 pins {ASSERTIONS_CSV_COLUMNS}"
    )
    return [dict(zip(header, row, strict=True)) for row in rows]


def shortest_paths(edges: Iterable[tuple[str, str]], source: str, target: str) -> list[list[str]]:
    """Every path of minimum hop count from ``source`` to ``target``.

    Enumerated rather than counted, so a test can say *which* two paths tie and a failure
    names them. Breadth-first by construction: the frontier only ever grows one hop, so
    the first depth that reaches the target holds every minimum path.
    """
    adjacency: dict[str, set[str]] = {}
    for a, b in edges:
        adjacency.setdefault(a, set()).add(b)
        adjacency.setdefault(b, set()).add(a)
    frontier: list[list[str]] = [[source]]
    seen = {source}
    while frontier:
        found = [path for path in frontier if path[-1] == target]
        if found:
            return found
        nxt: list[list[str]] = []
        arrived: set[str] = set()
        for path in frontier:
            for neighbour in sorted(adjacency.get(path[-1], ())):
                if neighbour not in seen:
                    nxt.append([*path, neighbour])
                    arrived.add(neighbour)
        seen |= arrived
        frontier = nxt
    return []


def membership(scenario: str, phase: str) -> dict[str, str]:
    """`record_key -> entity_label` from a committed membership expectation."""
    _, rows = rows_of(expected(scenario, phase, "membership.csv"))
    return {f"{system}:{record}": label for _, system, record, label in rows}


def edge_cuts(scenario: str, phase: str = BATCH) -> int:
    """The summed `edge_cut` count in a committed events expectation."""
    _, rows = rows_of(expected(scenario, phase, "events.csv"))
    return sum(int(count) for _, event_type, count in rows if event_type == EDGE_CUT)


def test_all_scenarios_pass_the_fixture_validator() -> None:
    """AC1: the S8.2.1 shape, over all six at once."""
    completed = subprocess.run(
        [
            sys.executable,
            str(REPO_ROOT / "scripts" / "validate_fixtures.py"),
            *(str(FIXTURE_ROOT / name) for name in SCENARIOS),
        ],
        capture_output=True,
        text=True,
        cwd=REPO_ROOT,
        check=False,
    )
    assert completed.returncode == 0, completed.stdout + completed.stderr


@pytest.mark.parametrize("scenario", SCENARIOS)
def test_expected_files_are_sorted_with_literal_headers(scenario: str) -> None:
    """AC1: byte-sorted on the full column tuple, literal headers, symbolic labels."""
    root = FIXTURE_ROOT / scenario / "expected"
    committed = sorted(root.rglob("*.csv"))
    assert committed, f"{scenario} commits no expectation at all"

    for path in committed:
        header, rows = rows_of(path)
        key = f"expected/<phase>/{path.name}"
        assert tuple(header) == EXPECTED_HEADERS[key], (
            f"{path} header is {tuple(header)}; S8.2.1 pins {EXPECTED_HEADERS[key]}"
        )
        assert rows, f"{path} carries a header and no rows; it claims nothing"
        assert rows == sorted(rows), (
            f"{path} is not byte-sorted on its column tuple.\n"
            f"  committed: {rows}\n  sorted:    {sorted(rows)}"
        )
        for row in rows:
            for field in row:
                assert field == field.strip(), f"{path} carries a padded field: {row}"
                assert field not in {"NULL", "null", "None", "NA"}, (
                    f"{path} spells a null as {field!r}; the only null token S8.2.1 "
                    f"admits is {NULL_TOKEN!r}"
                )
        if path.name == "membership.csv":
            for label in {row[3] for row in rows}:
                assert label.startswith("E") and label[1:].isdigit(), (
                    f"{path} carries entity_label {label!r}; M7 requires a symbolic "
                    "label, never a ULID, or the expectation is pinned to one run"
                )


def test_a1_a4_rows_and_phases() -> None:
    """AC2: exactly the four rows, canonical order, declared phases, A4 retracted."""
    rows = assertion_rows(MAIN)
    assert len(rows) == 4, f"{MAIN} declares {len(rows)} assertions; A1-A4 is four"

    pairs = [(row["rec_a_key"], row["rec_b_key"]) for row in rows]
    assert pairs == [A1, A2, A3, A4], f"{MAIN} asserts over {pairs}"
    for a, b in pairs:
        assert a < b, f"({a}, {b}) is not in canonical pair order (S5.0)"

    kinds = [row["kind"] for row in rows]
    assert kinds == [ALWAYS, NEVER, NEVER, ALWAYS], f"{MAIN} kinds are {kinds}"
    for row in rows:
        assert row["phase"] in PHASES, f"phase {row['phase']!r} is not one of S8.2.1's {PHASES}"

    # A4 is the retracted one, and the post-state is where that becomes visible.
    header, state = rows_of(expected(MAIN, BATCH, "assertions.csv"))
    by_pair = {(row[0], row[1]): row[3] for row in state}
    assert by_pair[A4] == "false", (
        f"A4 {A4} is active={by_pair[A4]!r} in the post-state; a retraction that left "
        "the row active would change the partition and nothing else would notice"
    )
    for pair in (A1, A2, A3):
        assert by_pair[pair] == "true", f"{pair} should still be active, got {by_pair[pair]!r}"


def test_a1_records_share_no_email_and_no_phone() -> None:
    """AC3: nothing but the `always` edge can join A1's two records."""
    people = records(MAIN, BASE)
    left, right = people[A1[0]], people[A1[1]]
    assert left["email"] and right["email"] and left["email"] != right["email"], (
        f"A1's records share the email {left['email']!r}; then the model could join "
        "them on its own and the `always` would prove nothing"
    )
    assert left["phone"] and right["phone"] and left["phone"] != right["phone"], (
        f"A1's records share the phone {left['phone']!r}; same problem"
    )
    assert not links(left, right), "A1's pair links without the assertion"

    assert membership(MAIN, BASE)[A1[0]] != membership(MAIN, BASE)[A1[1]], (
        "A1's records are already one entity in the base expectation"
    )
    assert membership(MAIN, BATCH)[A1[0]] == membership(MAIN, BATCH)[A1[1]], (
        "A1's `always` did not join its records in the batch expectation"
    )


def test_a2_splits_without_a_cut() -> None:
    """AC4: a `never` with no alternative path needs no partition-level cut."""
    people = records(MAIN, BASE)
    assert links(people[A2[0]], people[A2[1]]), (
        "A2's pair does not link naturally, so suppressing its edge proves nothing"
    )
    others = [key for key in people if key not in A2]
    for key in others:
        assert not (links(people[A2[0]], people[key]) and links(people[A2[1]], people[key])), (
            f"{key} connects both of A2's endpoints, so A2 would need a cut and would "
            "stop being the no-cut case"
        )
    batch = membership(MAIN, BATCH)
    assert batch[A2[0]] != batch[A2[1]], "A2's `never` did not separate its records"


def test_a3_expects_exactly_one_edge_cut() -> None:
    """AC5: the never pair stays connected through a third record, so one edge is cut."""
    people = records(MAIN, BASE)
    assert links(people[A3[0]], people[A3_THIRD]) and links(people[A3[1]], people[A3_THIRD]), (
        f"{A3_THIRD} does not link both of A3's endpoints, so no cut would be needed"
    )
    assert edge_cuts(MAIN) == 1, (
        f"{MAIN} expects {edge_cuts(MAIN)} edge_cut(s); A3 designs exactly one and A2 designs none"
    )

    batch = membership(MAIN, BATCH)
    assert batch[A3[0]] != batch[A3[1]], "A3's `never` did not separate its endpoints"
    with_third = [key for key in A3 if batch[key] == batch[A3_THIRD]]
    assert len(with_third) == 1, (
        f"{A3_THIRD} is co-clustered with {with_third}; after the cut it must remain "
        "with exactly one endpoint of the never pair"
    )


def test_cut_tie_edges_have_identical_agreement_patterns() -> None:
    """AC6: the two candidate edges are indistinguishable to the model."""
    rows = assertion_rows(CUT_TIE)
    assert len(rows) == 1 and rows[0]["kind"] == NEVER
    left, right = rows[0]["rec_a_key"], rows[0]["rec_b_key"]

    people = records(CUT_TIE, BASE)
    bridges = [key for key in people if key not in (left, right)]
    assert len(bridges) == 1, f"{CUT_TIE} has {len(bridges)} bridge records; it needs one"
    bridge = bridges[0]

    first = agreement_pattern(people[left], people[bridge])
    second = agreement_pattern(people[right], people[bridge])
    assert first == second, (
        f"the two path edges have different agreement patterns, so their probabilities "
        f"differ and the cut is decided by probability rather than by the tiebreak:\n"
        f"  ({left}, {bridge}): {first}\n  ({right}, {bridge}): {second}"
    )

    # With probability tied, `choose_cut_edge` orders by (probability, rec_a, rec_b), so
    # the cut edge is the lexicographically smaller canonical pair — and the record it
    # detaches is the one the committed expectation must place on its own.
    candidates = sorted((tuple(sorted((left, bridge))), tuple(sorted((right, bridge)))))
    cut = candidates[0]
    detached = left if left in cut else right
    batch = membership(CUT_TIE, BATCH)
    assert batch[detached] != batch[bridge], (
        f"the tiebreak cuts {cut}, which detaches {detached} from {bridge}, but the "
        f"committed expectation keeps them together"
    )
    assert edge_cuts(CUT_TIE) == 1, f"{CUT_TIE} expects {edge_cuts(CUT_TIE)} cuts, not 1"


def test_path_tie_has_two_equal_hop_paths() -> None:
    """AC7: exactly two minimum-hop paths connect the never pair."""
    rows = assertion_rows(PATH_TIE)
    assert len(rows) == 1 and rows[0]["kind"] == NEVER
    left, right = rows[0]["rec_a_key"], rows[0]["rec_b_key"]

    # The never pair's own edge is suppressed before the path search (S4.4.2), so it is
    # removed here too — leaving it in would make the shortest path one hop and the tie
    # would vanish.
    edges = designed_edges(PATH_TIE) - {tuple(sorted((left, right)))}
    paths = shortest_paths(edges, left, right)
    assert len(paths) == 2, (
        f"{PATH_TIE} designs {len(paths)} minimum-hop path(s) between {left} and "
        f"{right}: {paths}. D5's path tie needs exactly two of equal length."
    )
    assert len({len(path) for path in paths}) == 1, f"the two paths differ in length: {paths}"


def test_two_iterations_expects_two_edge_cuts() -> None:
    """AC7: cutting once leaves the pair connected, so the fixpoint runs twice."""
    rows = assertion_rows(TWO_ITERATIONS)
    assert len(rows) == 1 and rows[0]["kind"] == NEVER
    left, right = rows[0]["rec_a_key"], rows[0]["rec_b_key"]

    edges = designed_edges(TWO_ITERATIONS) - {tuple(sorted((left, right)))}
    paths = shortest_paths(edges, left, right)
    assert len(paths) >= 2, (
        f"{TWO_ITERATIONS} designs {len(paths)} path(s); with fewer than two the first "
        "cut disconnects the pair and one round is enough"
    )
    assert edge_cuts(TWO_ITERATIONS) == 2, (
        f"{TWO_ITERATIONS} expects {edge_cuts(TWO_ITERATIONS)} edge_cut(s); the whole "
        "point of the scenario is that a single-pass implementation stops one short"
    )


def test_contradiction_scenario_makes_no_post_state_claim() -> None:
    """AC8: the cycle is encoded, and the failing phase claims nothing."""
    rows = assertion_rows(CONTRADICTION)
    assert len(rows) == 3, f"{CONTRADICTION} declares {len(rows)} assertions; a cycle is three"

    kinds = sorted(row["kind"] for row in rows)
    assert kinds == [ALWAYS, ALWAYS, NEVER], f"{CONTRADICTION} kinds are {kinds}"

    pairs = {(row["rec_a_key"], row["rec_b_key"]): row["kind"] for row in rows}
    for a, b in pairs:
        assert a < b, f"({a}, {b}) is not in canonical pair order (S5.0)"
    nodes = {key for pair in pairs for key in pair}
    assert len(nodes) == 3, f"the cycle spans {sorted(nodes)}; it must span exactly three"
    assert len(pairs) == 3, "the three assertions must be over three DISTINCT pairs"

    # No pair may carry both kinds: S4.4 rejects the conflicting insert at write time, so
    # such a fixture could not be loaded at all.
    always_pairs = {pair for pair, kind in pairs.items() if kind == ALWAYS}
    never_pairs = {pair for pair, kind in pairs.items() if kind == NEVER}
    assert not (always_pairs & never_pairs), "a pair carries both an always and a never"

    assert not (FIXTURE_ROOT / CONTRADICTION / "expected" / BATCH).exists(), (
        f"{CONTRADICTION} commits an expected/{BATCH}/ directory. The batch run fails "
        "CONTRADICTION-1 (S4.4.1), so there is no post-state; S8.2.1 spells an absent "
        "claim as an absent file, not as an empty one."
    )
    assert (FIXTURE_ROOT / CONTRADICTION / "expected" / BASE / "membership.csv").exists(), (
        "the base phase succeeds and IS claimable; dropping it would under-claim"
    )


def test_non_batch_scenario_batch_excludes_asserted_records() -> None:
    """AC8: the affected set reaches records the batch never delivered (B5, S4.5.1)."""
    rows = assertion_rows(NON_BATCH)
    assert len(rows) == 1 and rows[0]["kind"] == NEVER
    asserted = {rows[0]["rec_a_key"], rows[0]["rec_b_key"]}

    delivered = set(records(NON_BATCH, BATCH))
    assert not (asserted & delivered), (
        f"the batch delivers {sorted(asserted & delivered)}, which the assertion names. "
        "The scenario exists to prove the affected set widens BEYOND the batch, so an "
        "asserted record in the batch would make it prove nothing."
    )
    assert delivered, "the batch delivers nothing at all, so the phase is not a real run"

    base, batch = membership(NON_BATCH, BASE), membership(NON_BATCH, BATCH)
    a, b = sorted(asserted)
    assert base[a] == base[b], "the asserted pair is not one entity before the assertion"
    assert batch[a] != batch[b], (
        "the asserted pair is still one entity after the batch, though the `never` "
        "applies — the affected set never reached them"
    )


@pytest.mark.parametrize("scenario", SCENARIOS)
def test_manifest_declares_assertions_as_an_input(scenario: str) -> None:
    """S8.2.1: `assertions.csv` is an INPUT the run loads, declared in `aux_files`."""
    loaded = load_scenario(scenario)
    assert loaded.phases == (BASE, BATCH), f"{scenario} declares phases {loaded.phases}"
    assert (FIXTURE_ROOT / scenario / "assertions.csv").exists(), (
        f"{scenario} is an assertion scenario with no assertions.csv"
    )


@pytest.mark.parametrize("scenario", SCENARIOS)
def test_no_pair_carries_both_kinds(scenario: str) -> None:
    """S4.4 rejects a conflicting insert at write time, so no fixture may encode one."""
    seen: dict[tuple[str, str], str] = {}
    for row in assertion_rows(scenario):
        pair = (row["rec_a_key"], row["rec_b_key"])
        assert pair not in seen or seen[pair] == row["kind"], (
            f"{scenario} commits both {seen.get(pair)!r} and {row['kind']!r} for {pair}. "
            "S4.4 rejects the second insert with exit 1, so the fixture could not load."
        )
        seen[pair] = row["kind"]


@pytest.mark.parametrize("scenario", SCENARIOS)
def test_no_golden_expectation_is_committed(scenario: str) -> None:
    """`golden_records` is first written in M4; a claim on it here is unassertable."""
    for phase in (BASE, BATCH):
        assert not expected(scenario, phase, "golden.csv").exists(), (
            f"{scenario}/expected/{phase}/golden.csv exists; S12 forbids a milestone "
            "asserting on a later milestone's relation"
        )
