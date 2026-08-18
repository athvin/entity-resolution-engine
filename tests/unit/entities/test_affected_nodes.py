"""The pure core of S4.5.1's node formula, over synthetic rows and no lake.

Every claim here is about the *algebra*: given a membership partition, an edge list and
an assertion set, which records the formula reaches. That is deliberately assertable
without a substrate — the seed arms are what need one, and they are
`tests/integration/test_affected_nodes.py`'s.

The two things this layer can prove that the integration layer cannot prove cheaply are
the boundary cases. A gray-band edge producing no partner is a claim about one
comparison, and pinning it against a corpus would mean finding a real pair that happens
to score inside the band. A seed record with no `entity_membership` row is a claim about
the "where assigned" of the formula, and reaching it against a real lake would mean
constructing a partition with a deliberate hole in it.

AC8's grep-clean check lives here too, for the same reason: it is a property of the
module's source, and a test that needed Docker to read a file would be paying for the
substrate twice.
"""

from __future__ import annotations

from datetime import datetime
from pathlib import Path
from typing import Final

import pytest

from er.entities import cluster
from er.entities.cluster import SEED_ARMS, AffectedSet, SeedRecords, affected_nodes, partners_of
from er.review.assertions import ALWAYS, NEVER, Assertion

#: `thresholds.auto_merge` for this module. A round number, because every probability
#: below is positioned relative to it and the arithmetic should be readable.
AUTO_MERGE: Final = 0.90

#: The lower edge of the half-open gray band (S4.3), for the pair that must NOT produce
#: a partner. Strictly below `AUTO_MERGE` and comfortably above any review threshold.
GRAY_BAND_PROBABILITY: Final = 0.85

#: The relation S4.3.4 calls the touched-subgraph driver and that the normative S4.5.1
#: formula nevertheless does not use. AC8: `cluster.py` must not mention it.
BLOCKING_KEYS_RELATION: Final = "int_blocking_keys"

#: One entity holding three records, of which only `crm:a` is ever seeded.
ENTITY_E: Final = "01JENTITYE0000000000000000"
ENTITY_F: Final = "01JENTITYF0000000000000000"

MEMBERSHIP: Final[dict[str, str]] = {
    "crm:a": ENTITY_E,
    "billing:b": ENTITY_E,
    "webforms:c": ENTITY_E,
    "crm:d": ENTITY_F,
    "billing:e": ENTITY_F,
}

CREATED_AT: Final = datetime(2026, 3, 1, 0, 0, 0)


def assertion(rec_a_key: str, rec_b_key: str, kind: str, *, active: bool = True) -> Assertion:
    """One `assertions` row, canonical per S5.0 and with no minted id to depend on.

    The `assertion_id` is derived from the pair rather than drawn from an
    :class:`~er.entities.ids.IdFactory`: nothing in this module reads it, and a counter
    would make the values depend on construction order.
    """
    assert rec_a_key < rec_b_key, f"({rec_a_key!r}, {rec_b_key!r}) is not canonical (S5.0)"
    return Assertion(
        assertion_id=f"{rec_a_key}|{rec_b_key}|{kind}",
        rec_a_key=rec_a_key,
        rec_b_key=rec_b_key,
        kind=kind,
        active=active,
        created_by="steward",
        created_at=CREATED_AT,
    )


def test_entity_widening_from_one_seed() -> None:
    """AC1: a seed record drags in every current member of its entity, and no more.

    The point of the arm. `billing:b` and `webforms:c` appear in no batch, carry no
    assertion and are the endpoints of no edge — they are affected solely because they
    share an entity with a record that changed, which is what makes S4.5.5's
    `member_removed` / `split` reachable in incremental mode.

    The negative half is asserted in the same test: entity F is untouched. A widening
    that returned the whole partition would satisfy the positive half alone.
    """
    result = affected_nodes(
        ["crm:a"],
        edges=[],
        membership=MEMBERSHIP,
        auto_merge=AUTO_MERGE,
    )

    assert result.seed == frozenset({"crm:a"})
    assert result.partners == frozenset()
    assert result.entities == frozenset({ENTITY_E})
    assert result.nodes == frozenset({"crm:a", "billing:b", "webforms:c"})
    assert "crm:d" not in result.nodes, (
        "entity F was reached from a seed that touches none of its members; the widening "
        "is one hop through entity membership, not a walk of the partition"
    )
    assert not result.is_empty


def test_partners_require_auto_merge() -> None:
    """AC2: the bound is `p >= auto_merge`, and the gray band produces nothing.

    Both sides of one comparison, because the interesting failure is an implementation
    that used `review_low` — or `>` instead of `>=` — and either mistake passes a test
    that only looks at a pair scoring 0.99. `crm:d` is reachable at exactly `auto_merge`;
    `billing:e` is reachable only through a banded edge and must stay out, along with the
    entity F membership that partnering it would have dragged in.
    """
    edges = [
        ("crm:a", "crm:d", AUTO_MERGE),
        ("crm:a", "billing:e", GRAY_BAND_PROBABILITY),
    ]

    partners = partners_of(["crm:a"], edges, auto_merge=AUTO_MERGE)

    assert partners == frozenset({"crm:d"}), (
        f"a pair at exactly {AUTO_MERGE} is a match and the band is half-open (S4.3); a "
        f"pair at {GRAY_BAND_PROBABILITY} is queued for review and is not clustered (S4.3.5)"
    )

    # And the consequence for the node set: entity F enters through `crm:d` alone, so
    # `billing:e` is affected as a CO-MEMBER and never as a partner.
    result = affected_nodes(["crm:a"], edges=edges, membership=MEMBERSHIP, auto_merge=AUTO_MERGE)
    assert result.partners == frozenset({"crm:d"})
    assert result.entities == frozenset({ENTITY_E, ENTITY_F})
    assert result.nodes == frozenset(MEMBERSHIP)

    # The falsification: with entity F's membership removed, the banded pair has no other
    # route in, and `billing:e` disappears from the node set entirely.
    unassigned = {key: entity for key, entity in MEMBERSHIP.items() if entity != ENTITY_F}
    narrowed = affected_nodes(["crm:a"], edges=edges, membership=unassigned, auto_merge=AUTO_MERGE)
    assert "billing:e" not in narrowed.nodes


def test_unassigned_seed_returns_itself() -> None:
    """AC1: a seed record with no `entity_membership` row yields itself plus partners.

    The "where assigned" clause of the formula. A record ingested for the first time has
    never been reconciled and so has no membership row; an implementation that joined
    membership inner-style would drop it from the affected set, and the very run that was
    supposed to create its entity would decline to consider it.
    """
    lone = affected_nodes(["crm:new"], edges=[], membership=MEMBERSHIP, auto_merge=AUTO_MERGE)

    assert lone.seed == frozenset({"crm:new"})
    assert lone.entities == frozenset()
    assert lone.nodes == frozenset({"crm:new"})
    assert not lone.is_empty, "an unassigned seed record is still work to do"

    # With a partner, the partner's entity widens even though the seed has none.
    joined = affected_nodes(
        ["crm:new"],
        edges=[("crm:a", "crm:new", 0.99)],
        membership=MEMBERSHIP,
        auto_merge=AUTO_MERGE,
    )
    assert joined.partners == frozenset({"crm:a"})
    assert joined.entities == frozenset({ENTITY_E})
    assert joined.nodes == frozenset({"crm:new", "crm:a", "billing:b", "webforms:c"})


def test_always_assertion_creates_a_partner() -> None:
    """AC2: an `always` partners two records with no scored edge; a `never` withdraws one.

    The steward path, and the reason the partner rule is defined over the
    *assertion-adjusted* edge set rather than over `match_scores`. Two records that no
    blocking rule ever paired have no row to read, so an unadjusted partner rule would
    make a steward-authored merge unreachable — the assertion would be durable (S4.4) and
    inert.

    The `never` half is the same claim in reverse: an edge scored at 1.0 supplies no
    partner once a steward has said the two are not the same person.
    """
    always = affected_nodes(
        ["crm:a"],
        edges=[],
        membership=MEMBERSHIP,
        auto_merge=AUTO_MERGE,
        assertions=[assertion("crm:a", "crm:d", ALWAYS)],
    )

    assert always.partners == frozenset({"crm:d"}), (
        "an active `always` inserts an edge at p = 1.0 into the clustering edge set "
        "(S4.4); nothing was ever scored for this pair"
    )
    assert always.entities == frozenset({ENTITY_E, ENTITY_F})
    assert always.nodes == frozenset(MEMBERSHIP)

    # Retracted, it is not a constraint: the seed arm still reports the pair, but the
    # edge set is unadjusted and the partner is gone.
    retracted = partners_of(
        ["crm:a"],
        [],
        auto_merge=AUTO_MERGE,
        assertions=[assertion("crm:a", "crm:d", ALWAYS, active=False)],
    )
    assert retracted == frozenset()

    # `never` removes the edge regardless of score (S4.4).
    denied = partners_of(
        ["crm:a"],
        [("crm:a", "crm:d", 0.99)],
        auto_merge=AUTO_MERGE,
        assertions=[assertion("crm:a", "crm:d", NEVER)],
    )
    assert denied == frozenset()


def test_empty_seed_is_an_empty_affected_set() -> None:
    """AC7's pure half: nothing seeded, nothing affected — edges and assertions or not.

    The unit-level statement of what `er reconcile` exits ``10`` on. Asserted with a
    populated edge set and an active `always` present, because the failure it guards
    against is a partner rule that iterates the edge set rather than the seed and so
    reports every scored record as affected on a run that changed nothing.
    """
    result = affected_nodes(
        [],
        edges=[("crm:a", "crm:d", 0.99)],
        membership=MEMBERSHIP,
        auto_merge=AUTO_MERGE,
        assertions=[assertion("billing:b", "billing:e", ALWAYS)],
    )

    assert result == AffectedSet(
        seed=frozenset(), partners=frozenset(), entities=frozenset(), nodes=frozenset()
    )
    assert result.is_empty


def test_seed_arms_are_named_and_unioned() -> None:
    """Every arm of S4.5.1 is separately addressable, and `records` is their union.

    :data:`SEED_ARMS` is what the stage's counters and a human diagnosis read; a union
    computed over a hard-coded four would drop an arm silently, which is the exact defect
    this ticket exists to repair.
    """
    seed = SeedRecords(
        batch=frozenset({"crm:a"}),
        content_hash_delta=frozenset({"billing:b"}),
        deletions=frozenset({"webforms:c"}),
        assertion_delta=frozenset({"crm:d"}),
        review_delta=frozenset({"billing:e"}),
    )

    assert set(SEED_ARMS) == set(vars(seed))
    assert seed.records == frozenset(MEMBERSHIP)
    assert seed.arm_sizes() == dict.fromkeys(SEED_ARMS, 1)

    empty = SeedRecords(*(frozenset() for _ in SEED_ARMS))
    assert empty.records == frozenset()


def test_cluster_module_never_reads_blocking_keys() -> None:
    """AC8: `cluster.py` contains no reference to `int_blocking_keys`.

    S4.3.4 names that relation the driver of the touched-subgraph computation, which is
    a standing invitation to join it into the node set — and doing so would widen the
    affected set to every record sharing a blocking key, most of which score nowhere near
    `auto_merge`. The normative formula in S4.5.1 is edge-based, so the grep is the
    check.

    Textual and over the whole file, comments included: a mention in a docstring is how a
    later ticket talks itself into adding the join, and the module says why it does not
    use the relation without naming it in a form this test would tolerate.
    """
    source = Path(cluster.__file__).read_text(encoding="utf-8")
    lines = [
        f"{number}: {line.strip()}"
        for number, line in enumerate(source.splitlines(), start=1)
        if BLOCKING_KEYS_RELATION in line
    ]
    assert lines == [], (
        f"src/er/entities/cluster.py names {BLOCKING_KEYS_RELATION}: "
        + "; ".join(lines)
        + ". The partner rule reads the assertion-adjusted edge set alone (S4.5.1)"
    )


@pytest.mark.parametrize("probability", [AUTO_MERGE, 1.0, 0.999])
def test_partner_bound_is_inclusive_at_and_above_auto_merge(probability: float) -> None:
    """The inclusive bound, at the edge and above it (S4.3, S4.5.1)."""
    assert partners_of(["crm:a"], [("crm:a", "crm:d", probability)], auto_merge=AUTO_MERGE) == (
        frozenset({"crm:d"})
    )
