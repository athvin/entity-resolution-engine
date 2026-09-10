"""Fixture lint for `deletion_scenario` (S8.2, S8.2.1, S4.1.1, S4.5.5).

The scenario's whole value is what its `refresh/` delivery LEAVES OUT: exactly two
live keys, one of which is the bridge record whose removal must force a `split` and
the other a singleton whose removal must `retire` its entity. Both properties are
claims about the committed CSVs, so they are derived here rather than asserted in
prose — a value edit that dissolves the bridge fails this file, not a confusing
integration diff hours later (AC6).

**The edge rule.** As in `tests/unit/fixtures/test_split_scenario.py`, an edge is
designed between two records iff they share an exact identifier — a normalized
email or phone. Its soundness premises for THIS fixture are machine-checked below:
every identifier-sharing pair also agrees on given name, family name and at least
one of birth_date / postal (which clears `auto_merge` under `model_test_v1` with
bits to spare), and every remaining pair shares **no S6 blocking key**, so it is
never a candidate and cannot be an edge under any threshold.
"""

from __future__ import annotations

from collections.abc import Iterable, Mapping
from datetime import datetime
from pathlib import Path
from typing import Final

from helpers.scenario import EXPECTED_HEADERS, load_scenario

from er.config.loader import load_config
from er.config.schema import Config

REPO_ROOT: Final = Path(__file__).resolve().parents[3]
FIXTURE_ROOT: Final = REPO_ROOT / "fixtures" / "static"
CONFIG_PATH: Final = REPO_ROOT / "configs" / "test.yaml"

SCENARIO: Final = "deletion_scenario"

BASE: Final = "base"
REFRESH: Final = "refresh"
RESURRECT: Final = "resurrect"

#: The fields the S6 comparisons read, in the spelling `sources.<name>.columns` maps.
COMPARED: Final = ("given_name", "family_name", "email", "phone", "birth_date", "addr_postal")

#: `name_postal` and `dob_name` as S6 writes them.
FAMILY_PREFIX: Final = 4
GIVEN_PREFIX: Final = 3

#: How many live keys the refresh delivery must omit (S8.2's designed count).
OMITTED: Final = 2


def cfg() -> Config:
    return load_config(CONFIG_PATH)


def rows_of(path: Path) -> tuple[list[str], list[list[str]]]:
    """`(header, rows)` as raw split fields — no null decoding, no re-sorting."""
    lines = path.read_text(encoding="utf-8").splitlines()
    assert lines, f"{path} is empty"
    return lines[0].split(","), [line.split(",") for line in lines[1:] if line]


def records(phase: str) -> dict[str, dict[str, str]]:
    """`record_key -> {logical field: comparable value}` for one phase's inputs."""
    document = cfg()
    out: dict[str, dict[str, str]] = {}
    directory = FIXTURE_ROOT / SCENARIO / phase
    for path in sorted(directory.glob("*.csv")):
        spec = document.sources[path.stem]
        header, rows = rows_of(path)
        for row in rows:
            raw = dict(zip(header, row, strict=True))
            key = f"{path.stem}:{raw[spec.record_id_column]}"
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


def personas(phase: str) -> dict[str, str]:
    """`record_key -> persona_id` from the delivered CSVs (S8.2's truth column)."""
    labels: dict[str, str] = {}
    directory = FIXTURE_ROOT / SCENARIO / phase
    for path in sorted(directory.glob("*.csv")):
        spec = cfg().sources[path.stem]
        header, rows = rows_of(path)
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
    """Every canonical pair sharing an exact identifier — the module's edge rule."""
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


def omitted_keys() -> tuple[set[str], dict[str, dict[str, str]]]:
    """The live keys `refresh/` leaves out, and the base records for context."""
    base = records(BASE)
    refreshed = set(records(REFRESH))
    return set(base) - refreshed, base


def test_refresh_omits_two_keys_one_of_which_is_a_bridge() -> None:
    """AC6: exactly two omissions; the multi-member one is a cut vertex; the other
    is a singleton whose tombstone empties its entity."""
    omitted, base = omitted_keys()
    assert len(omitted) == OMITTED, (
        f"refresh/ omits {sorted(omitted)} — S8.2 designs exactly {OMITTED} omissions, "
        "one bridge and one entity-emptying singleton"
    )

    # The edge rule's soundness premises, checked rather than assumed.
    edges = designed_edges(base)
    for a, b in sorted(edges):
        left, right = base[a], base[b]
        assert (left["given_name"], left["family_name"]) == (
            right["given_name"],
            right["family_name"],
        ), f"identifier-sharing pair ({a}, {b}) disagrees on name; the edge rule is unsound"
        assert (
            left["birth_date"] == right["birth_date"] or left["addr_postal"] == right["addr_postal"]
        ), f"({a}, {b}) agrees on neither birth_date nor postal; the edge rule is unsound"
    for a in sorted(base):
        for b in sorted(base):
            if a < b and (a, b) not in edges:
                assert not shared_blocking_keys(base[a], base[b]), (
                    f"({a}, {b}) is not a designed edge but shares "
                    f"{shared_blocking_keys(base[a], base[b])}; it would be scored and "
                    "could carry the component around the bridge"
                )

    labels = personas(BASE)
    by_size = sorted(omitted, key=lambda key: sum(1 for k in labels if labels[k] == labels[key]))
    singleton, bridge = by_size[0], by_size[-1]
    assert sum(1 for key in labels if labels[key] == labels[singleton]) == 1, (
        f"{singleton} is not a singleton persona; nothing in the fixture reaches "
        "the `retired` path (S4.5.5)"
    )

    persona_members = frozenset(key for key in labels if labels[key] == labels[bridge])
    persona_edges = {(a, b) for a, b in edges if a in persona_members and b in persona_members}
    whole = components(persona_members, persona_edges)
    assert whole == [persona_members], "the bridge's persona is not one designed component"

    survivors = persona_members - {bridge}
    surviving_edges = {(a, b) for a, b in persona_edges if bridge not in (a, b)}
    severed = components(survivors, surviving_edges)
    assert len(severed) == 2 and all(severed), (
        f"removing {bridge} leaves {len(severed)} part(s) {severed}; the omitted key "
        "designated as the bridge is not a cut vertex of its persona's edge graph, "
        "so no `split` can happen and the scenario proves nothing (AC6)"
    )

    # The resurrection re-appears the BRIDGE, unchanged, and only the bridge:
    # re-delivering it is what re-merges the two fragments (S4.1.1, S4.5.3).
    resurrected = records(RESURRECT)
    assert set(resurrected) == {bridge}, (
        f"resurrect/ delivers {sorted(resurrected)}; the scenario re-appears exactly the bridge key"
    )
    assert resurrected[bridge] == base[bridge], (
        "the resurrected record's values differ from the base delivery; resurrection "
        "is an ordinary re-appearance, not a supersession (S4.1.1)"
    )


def test_phase_dirs_and_expected_headers_match_s8_2_1() -> None:
    """The three-phase layout, literal headers, byte-sorted expectations."""
    scenario = load_scenario(SCENARIO)
    assert scenario.phases == (BASE, REFRESH, RESURRECT), (
        f"phases are {scenario.phases}; S8.2 pins base -> refresh -> resurrect"
    )

    root = FIXTURE_ROOT / SCENARIO / "expected"
    committed = sorted(root.rglob("*.csv"))
    assert {path.parent.name for path in committed} == {BASE, REFRESH, RESURRECT}, (
        "every phase makes at least one claim"
    )
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

    golden = list(root.rglob("golden.csv"))
    assert not golden, f"{golden} committed before M4 (S12 gating)"

    # The tombstoned-and-never-resurrected key appears in no expectation after base:
    # S4.2 excludes it from the corpus, so a file that still names it is claiming a
    # membership row the pipeline must not hold (AC7's "absent from both sides").
    omitted, _ = omitted_keys()
    resurrected = set(records(RESURRECT))
    gone = omitted - resurrected
    for phase in (REFRESH, RESURRECT):
        _, rows = rows_of(root / phase / "membership.csv")
        present = {f"{system}:{record}" for _, system, record, _ in rows}
        assert not present & gone, (
            f"expected/{phase}/membership.csv names tombstoned key(s) {present & gone}"
        )
