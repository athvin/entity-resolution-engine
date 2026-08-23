"""The five S4.6 survivorship rules, the chain, and the deciding-rule attribution.

M11 found five rules named and none defined, with chains that are not total orders — so
the golden winner depended on physical row order, which differs between the
touched-subset and the full-corpus materialisation. These tests hold the macros to
S4.6 in two different ways, and both are needed:

* **The fragments are compared as strings.** Each rule renders to exactly the literal
  `ORDER BY` fragment in the S4.6 table. A diff against that table is a test failure,
  not a review comment, which is what stops `recency` quietly becoming bare
  `updated_at_source` — a change that is invisible in every passing scenario until a
  row has a NULL there.
* **The behaviour is executed.** A string comparison cannot show that a chain actually
  selects the right row, so every rule also gets a designed two-row tie in which only
  that rule's key differs, run through DuckDB.

Every case is additionally run with the input rows inserted in the opposite physical
order. That arm is the whole point of the ticket: a chain that is a total order gives
the same winner and the same deciding rule regardless of row order, and one that is not
does not. Without it the suite would pass on chains missing the mandatory terminal
`record_key ASC`.

The member rows are in the LONG form S4.6's fragments assume — one row per
`(entity_id, attribute, value)` — which is why `value` is a real column here. `sources`
is a `MAP(VARCHAR, STRUCT(priority_rank INTEGER))` built from `configs/test.yaml`, so
`sources[source_system].priority_rank` is a real lookup and the priority ranks under
test are the ones the pipeline actually uses (S6).

This module builds its own harness: the session one in `conftest.py` registers nothing,
and a test that creates relations mutates harness state.
"""

from __future__ import annotations

import re
from collections.abc import Iterator, Mapping, Sequence
from pathlib import Path
from typing import Any, Final

import pytest
from harness import MacroHarness
from jinja2 import UndefinedError

from er.config.schema import Config

REPO_ROOT: Final = Path(__file__).resolve().parents[3]
MACRO_ROOT: Final = REPO_ROOT / "dbt" / "macros"

#: The S4.6 "Literal `ORDER BY` fragment" table, transcribed. `<attr>` is the only
#: placeholder the spec marks, and it is substituted for the attribute under test;
#: `value` carries no angle brackets and is a real column of the long-form member rows.
S4_6_FRAGMENTS: Final[Mapping[str, str]] = {
    "source_priority": "sources[source_system].priority_rank ASC",
    "recency": "COALESCE(updated_at_source, ingested_at) DESC",
    "frequency": "count(*) OVER (PARTITION BY entity_id, value) DESC",
    "completeness": "(value IS NOT NULL) DESC, length(value) DESC",
    "validated": "<attr>_valid DESC NULLS LAST",
}

#: S4.6's mandatory terminal element, and S5's closed name for it having decided.
TERMINAL: Final = "record_key ASC"
TIEBREAK: Final = "tiebreak_deterministic"

#: The attribute every case is rendered for. `email` because it is the one attribute
#: whose chain in `configs/test.yaml` exercises all three of `validated`,
#: `source_priority` and `recency`.
ATTRIBUTE: Final = "email"

#: The long-form member-row columns the S4.6 fragments read, with their types. `sources`
#: is added separately: DuckDB has no literal syntax for a MAP column in a VALUES list.
MEMBER_COLUMNS: Final[tuple[tuple[str, str], ...]] = (
    ("entity_id", "VARCHAR"),
    ("value", "VARCHAR"),
    ("record_key", "VARCHAR"),
    ("source_system", "VARCHAR"),
    ("email_valid", "BOOLEAN"),
    ("updated_at_source", "TIMESTAMP"),
    ("ingested_at", "TIMESTAMP"),
)

MEMBER_RELATION: Final = "member_rows"

#: The chain under test for the executed arms. Its three rules are checked one at a
#: time by designed ties, so the chain being longer than any single case needs is the
#: point: a rule must decide *through* the ones ahead of it tying.
EMAIL_CHAIN: Final[tuple[str, ...]] = ("validated", "source_priority", "recency")


def normalise(sql: str) -> str:
    """Collapse whitespace so a fragment compares as S4.6 writes it, not as it wraps."""
    return re.sub(r"\s+", " ", sql).strip()


@pytest.fixture
def harness() -> Iterator[MacroHarness]:
    """A harness of this module's own, because these tests create relations."""
    built = MacroHarness(macro_root=MACRO_ROOT, vars={})
    try:
        yield built
    finally:
        built.close()


def sources_map(cfg: Config) -> str:
    """`configs/test.yaml`'s sources as the MAP literal the S4.6 fragment indexes."""
    entries = ", ".join(
        f"'{name}': {{'priority_rank': {spec.priority_rank}}}"
        for name, spec in sorted(cfg.sources.items())
    )
    return "MAP{" + entries + "}"


def load_members(
    harness: MacroHarness, cfg: Config, rows: Sequence[Mapping[str, Any]], *, reverse: bool = False
) -> None:
    """(Re)create `member_rows` from ``rows``, optionally in reversed physical order.

    The reversal is how AC6 is exercised: the same logical input, a different order on
    disk. A chain that is a total order cannot tell the difference.
    """
    ordered = list(reversed(rows)) if reverse else list(rows)
    columns = ", ".join(f"{name} {sql_type}" for name, sql_type in MEMBER_COLUMNS)
    harness.execute(f"DROP TABLE IF EXISTS {MEMBER_RELATION}")
    harness.execute(f"CREATE TABLE {MEMBER_RELATION} ({columns})")
    placeholders = ", ".join("?" for _ in MEMBER_COLUMNS)
    for row in ordered:
        harness.execute(
            f"INSERT INTO {MEMBER_RELATION} VALUES ({placeholders})",
            [row.get(name) for name, _ in MEMBER_COLUMNS],
        )
    harness.execute(
        f"ALTER TABLE {MEMBER_RELATION} ADD COLUMN sources "
        "MAP(VARCHAR, STRUCT(priority_rank INTEGER))"
    )
    harness.execute(f"UPDATE {MEMBER_RELATION} SET sources = {sources_map(cfg)}")


def decide(
    harness: MacroHarness,
    cfg: Config,
    rows: Sequence[Mapping[str, Any]],
    chain: Sequence[str] = EMAIL_CHAIN,
    *,
    reverse: bool = False,
) -> dict[str, tuple[str, str]]:
    """Run `survivorship_decision` over ``rows`` -> entity_id -> (winning value, rule)."""
    load_members(harness, cfg, rows, reverse=reverse)
    sql = harness.render_macro("survivorship_decision", ATTRIBUTE, list(chain), MEMBER_RELATION)
    cursor = harness.execute(sql)
    return {
        str(entity_id): (str(value), str(rule))
        for entity_id, value, _record_key, rule in cursor.fetchall()
    }


def member(
    value: str | None,
    record_key: str,
    source_system: str = "crm",
    *,
    email_valid: bool | None = True,
    updated_at_source: str | None = "2024-01-01 00:00:00",
    ingested_at: str = "2024-06-01 00:00:00",
    entity_id: str = "E1",
) -> dict[str, Any]:
    """One member row, with every field a designed tie does not vary held equal."""
    return {
        "entity_id": entity_id,
        "value": value,
        "record_key": record_key,
        "source_system": source_system,
        "email_valid": email_valid,
        "updated_at_source": updated_at_source,
        "ingested_at": ingested_at,
    }


def test_each_rule_fragment_is_literal(harness: MacroHarness) -> None:
    """AC1: every rule renders exactly its row of the S4.6 table."""
    for rule, expected in S4_6_FRAGMENTS.items():
        rendered = harness.render_macro(f"rule_{rule}", ATTRIBUTE)
        assert normalise(rendered) == normalise(expected.replace("<attr>", ATTRIBUTE)), (
            f"rule_{rule} renders {rendered!r}; S4.6's table gives "
            f"{expected!r}. The fragment is the spec, so a diff here is a defect in "
            "the macro, not a formatting preference"
        )


def test_chain_terminates_in_record_key_asc_exactly_once(harness: MacroHarness) -> None:
    """AC2: the chain concatenates in order and the terminal appears exactly once.

    Both arms matter. S6.1's normalization already appends `record_key ASC`, so the
    macro receives chains with and without it and must produce the same total order —
    appending blindly would emit it twice, which sorts identically and is therefore a
    defect no scenario test could ever surface.
    """
    rendered = harness.render_macro("survivorship_order_by", ATTRIBUTE, list(EMAIL_CHAIN))
    expected = ", ".join(
        [
            S4_6_FRAGMENTS["validated"].replace("<attr>", ATTRIBUTE),
            S4_6_FRAGMENTS["source_priority"],
            S4_6_FRAGMENTS["recency"],
            TERMINAL,
        ]
    )
    assert normalise(rendered) == normalise(expected)

    already = harness.render_macro("survivorship_order_by", ATTRIBUTE, [*EMAIL_CHAIN, TERMINAL])
    assert normalise(already) == normalise(expected), (
        "a chain that already carries the terminal element rendered differently from "
        "one that does not; S6.1 appends it during normalization, so the macro must be "
        "idempotent about it"
    )
    assert already.count(TERMINAL) == 1, f"{TERMINAL!r} appears twice in {already!r}"


def test_source_priority_decides_designed_tie(harness: MacroHarness, test_config: Config) -> None:
    """AC3: `validated` ties, so the lower `priority_rank` wins and is reported."""
    rows = [
        member("billing@x.com", "billing:B1", "billing"),
        member("crm@x.com", "crm:C1", "crm"),
    ]
    for reverse in (False, True):
        decided = decide(harness, test_config, rows, reverse=reverse)
        assert decided["E1"] == ("crm@x.com", "source_priority"), (
            f"crm has priority_rank {test_config.sources['crm'].priority_rank} and "
            f"billing {test_config.sources['billing'].priority_rank}; "
            f"reverse={reverse} gave {decided['E1']}"
        )


def test_recency_coalesces_ingested_at(harness: MacroHarness, test_config: Config) -> None:
    """AC5: a NULL `updated_at_source` falls back to `ingested_at`, and can win.

    This is the arm that pins `recency` to `COALESCE(updated_at_source, ingested_at)`
    rather than to bare `updated_at_source`: under the bare column the NULL row sorts
    last under `DESC` and this test's winner flips.
    """
    rows = [
        member(
            "older@x.com",
            "crm:C1",
            updated_at_source="2024-02-01 00:00:00",
            ingested_at="2024-02-01 00:00:00",
        ),
        member(
            "fallback@x.com",
            "crm:C2",
            updated_at_source=None,
            ingested_at="2024-09-01 00:00:00",
        ),
    ]
    for reverse in (False, True):
        decided = decide(harness, test_config, rows, reverse=reverse)
        assert decided["E1"] == ("fallback@x.com", "recency"), (
            "the row whose COALESCE resolves to the later ingested_at must win "
            f"(reverse={reverse}, got {decided['E1']})"
        )


def test_frequency_picks_modal_value(harness: MacroHarness, test_config: Config) -> None:
    """AC3: `count(*) OVER (PARTITION BY entity_id, value)` prefers the modal value.

    `frequency` needs a differently shaped case from the other four, and the reason is
    structural rather than incidental. It is a PARTITION-level aggregate, so a value
    held by two rows gives both of them the same key — meaning those two rows are
    rank-1 and rank-2, their `frequency` keys tie, and the terminal element is what
    actually chose between them. `tiebreak_deterministic` is the honest answer there,
    and a test asserting `frequency` would be asserting a lie about which rule picked
    the winning *record*.

    So the case is built with three rows: two carrying the modal value and one carrying
    a singleton, with `validated` demoting the second modal row below the singleton.
    Rank-1 and rank-2 are then the modal row and the singleton row, they tie on
    `validated`, and `frequency` is the first rule that separates them — which is
    exactly what AC3 asks for.
    """
    chain = ("validated", "frequency")
    rows = [
        member("modal@x.com", "crm:C1", email_valid=True),
        member("modal@x.com", "crm:C2", email_valid=False),
        member("once@x.com", "crm:C0", email_valid=True),
    ]
    for reverse in (False, True):
        decided = decide(harness, test_config, rows, chain, reverse=reverse)
        # `crm:C0` sorts first on the terminal element, so a chain that failed to apply
        # `frequency` would pick `once@x.com` — the case is designed so the two answers
        # differ rather than coinciding.
        assert decided["E1"] == ("modal@x.com", "frequency"), (
            f"the value held by two rows must win (reverse={reverse}, got {decided['E1']})"
        )

    # The other half of the same fact: when both modal rows outrank the singleton, they
    # are rank-1 and rank-2, nothing in the chain separates them, and the terminal
    # element decides the record even though `frequency` decided the value.
    modal_only = [
        member("modal@x.com", "crm:C2"),
        member("modal@x.com", "crm:C1"),
        member("once@x.com", "crm:C0"),
    ]
    decided = decide(harness, test_config, modal_only, ("frequency",))
    assert decided["E1"] == ("modal@x.com", TIEBREAK), (
        "with both modal rows on top, the deciding rule for the winning RECORD is the "
        f"terminal element, not frequency (got {decided['E1']})"
    )


def test_completeness_prefers_non_null_then_longer(
    harness: MacroHarness, test_config: Config
) -> None:
    """AC3: `(value IS NOT NULL) DESC, length(value) DESC`, in that order."""
    chain = ("completeness",)
    nulls = [member(None, "crm:C0"), member("present@x.com", "crm:C1")]
    for reverse in (False, True):
        decided = decide(harness, test_config, nulls, chain, reverse=reverse)
        assert decided["E1"] == ("present@x.com", "completeness"), (
            f"a non-NULL value must beat a NULL one (reverse={reverse})"
        )

    lengths = [member("short@x.com", "crm:C0"), member("a-longer-address@x.com", "crm:C1")]
    for reverse in (False, True):
        decided = decide(harness, test_config, lengths, chain, reverse=reverse)
        assert decided["E1"] == ("a-longer-address@x.com", "completeness"), (
            f"the longer value must win when both are non-NULL (reverse={reverse})"
        )


def test_validated_orders_valid_first_nulls_last(
    harness: MacroHarness, test_config: Config
) -> None:
    """AC3: `<attr>_valid DESC NULLS LAST` — true, then false, then NULL."""
    chain = ("validated",)
    rows = [
        member("unknown@x.com", "crm:C0", email_valid=None),
        member("invalid@x.com", "crm:C1", email_valid=False),
        member("valid@x.com", "crm:C2", email_valid=True),
    ]
    for reverse in (False, True):
        decided = decide(harness, test_config, rows, chain, reverse=reverse)
        assert decided["E1"] == ("valid@x.com", "validated"), (
            f"the valid row must win (reverse={reverse}, got {decided['E1']})"
        )

    # NULLS LAST is the half a plain `DESC` would get wrong: with the valid row gone,
    # `false` must still beat `NULL`.
    without_valid = rows[:2]
    for reverse in (False, True):
        decided = decide(harness, test_config, without_valid, chain, reverse=reverse)
        assert decided["E1"] == ("invalid@x.com", "validated"), (
            f"email_valid=false must sort before NULL (reverse={reverse})"
        )


def test_full_tie_reports_tiebreak_deterministic(
    harness: MacroHarness, test_config: Config
) -> None:
    """AC4: every rule tying selects the lexically smaller `record_key`.

    And a single candidate reports the same rule: there was no rival, so nothing in the
    chain decided and the terminal element did — which is exactly what S5's closed
    `golden_lineage.rule` vocabulary spells `tiebreak_deterministic`.
    """
    tied = [member("same@x.com", "crm:C2"), member("same@x.com", "crm:C1")]
    for reverse in (False, True):
        decided = decide(harness, test_config, tied, reverse=reverse)
        assert decided["E1"] == ("same@x.com", TIEBREAK), (
            f"a full tie must be decided by record_key ASC (reverse={reverse}, got {decided['E1']})"
        )

    alone = [member("only@x.com", "crm:C9")]
    decided = decide(harness, test_config, alone)
    assert decided["E1"] == ("only@x.com", TIEBREAK), (
        "a single candidate has no rank-2 row, so no chain rule can have decided"
    )


def test_winner_invariant_under_row_shuffle(harness: MacroHarness, test_config: Config) -> None:
    """AC6: neither the winner nor the reported rule depends on physical row order.

    The chain is a total order or it is not, and this is the assertion that says which.
    It runs over several entities at once so that the partitioning is exercised too —
    a chain that happened to work on one entity could still leak across partitions.
    """
    rows = [
        member("crm@x.com", "crm:C1", "crm", entity_id="E1"),
        member("billing@x.com", "billing:B1", "billing", entity_id="E1"),
        member("same@x.com", "webforms:W2", "webforms", entity_id="E2"),
        member("same@x.com", "webforms:W1", "webforms", entity_id="E2"),
        member("solo@x.com", "crm:C3", "crm", entity_id="E3"),
    ]
    forward = decide(harness, test_config, rows)
    backward = decide(harness, test_config, rows, reverse=True)

    assert forward == backward, (
        f"reversing the physical row order changed the outcome:\n"
        f"  forward:  {forward}\n  reversed: {backward}\n"
        "The chain is therefore not a total order, which is design gap M11"
    )
    assert forward["E1"] == ("crm@x.com", "source_priority")
    assert forward["E2"] == ("same@x.com", TIEBREAK)
    assert forward["E3"] == ("solo@x.com", TIEBREAK)


def test_unknown_rule_raises(harness: MacroHarness) -> None:
    """AC7: an unknown rule fails the render, and the message names it.

    Naming it is the requirement: a chain is config-authored (S6), so the operator who
    typed `recencey` needs the render to say so rather than to report that some rule
    somewhere was not found.
    """
    with pytest.raises(UndefinedError) as refusal:
        harness.render_macro("survivorship_order_by", ATTRIBUTE, ["recencey"])
    assert "recencey" in str(refusal.value), (
        f"the failure does not name the offending rule: {refusal.value}"
    )
    for known in S4_6_FRAGMENTS:
        assert known in str(refusal.value), (
            f"the failure should list the valid rules; {known!r} is missing from {refusal.value}"
        )
