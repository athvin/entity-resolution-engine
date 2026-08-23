"""The reconciler as a pure function (S4.5.3, S4.5.4, S8.4), over synthetic partitions.

S8.4 names eight reconciler cases and asks for them "with an injectable `IdFactory` ...
on synthetic partitions". Every one of them is a claim about the overlap matrix and
nothing else, so the whole file runs on a bare runner: there is no lake, no clock and no
`run_id` anywhere below, and the only ids in play are the ones
:class:`~er.entities.ids.CountingIdFactory` hands out in a sequence that is identical in
every process.

The eight cases, and where each is asserted:

* unassigned-only cluster — ``test_unassigned_only_cluster_mints_one_entity_per_group``
* extend — ``test_member_added_and_member_removed_events``
* merge — ``test_claimant_tiebreak_by_min_record_key_in_overlap``
* split — ``test_two_two_split_resolved_by_min_record_key``
* merge-and-split-at-once — ``test_merge_and_split_at_once``
* a 2–2 split resolved by `min record_key ASC` — ``test_two_two_split_resolved_by_min_record_key``
* empty fragment → `retired` — ``test_empty_fragment_retires_entity``
* a record leaving all clusters → singleton — ``test_record_leaving_all_clusters_becomes_singleton``

The remaining two S8.4 bullets — redirect chains three deep, and a redirect cycle that
MUST raise — are :func:`er.entities.ids.resolve`'s and are asserted by ER-013's tests;
this module records `merged_into` edges and never follows one.

`record_key` values are spelled `crm:N` rather than as opaque labels because every
tiebreak in S4.5.3 is lexical on exactly that string, and a fixture whose ordering is not
visible in the fixture is a fixture that cannot be reviewed.
"""

from __future__ import annotations

import ast
from collections.abc import Collection, Iterable, Mapping
from pathlib import Path
from typing import Final

import pytest

from er.entities import reconcile
from er.entities.events import EVENT_TYPES, EventLog
from er.entities.ids import CountingIdFactory
from er.entities.reconcile import (
    ACTIVE,
    CREATED,
    MEMBER_ADDED,
    MEMBER_REMOVED,
    MERGED,
    PLANNED_STATUSES,
    RECLUSTER,
    RETIRED,
    SPLIT,
    PartitionError,
    ReconcilePlan,
    fragment_rank,
    overlap_matrix,
    reconcile_plan,
)
from er.lake.model import ENTITY_STATUSES

#: Entity ids of the prior partition. ULID-shaped and lexically ordered E1 < E2 < E3, so
#: that a test asserting a tiebreak cannot be satisfied by an accidental ordering of the
#: labels themselves.
E1: Final = "01JENTITY10000000000000000"
E2: Final = "01JENTITY20000000000000000"
E3: Final = "01JENTITY30000000000000000"

#: The first four ids a fresh :class:`CountingIdFactory` hands out. Materialised once so
#: a test can name the id a particular group must have received without reimplementing
#: the factory's rendering.
_MINT_PROBE: Final = CountingIdFactory()
MINT_SEQUENCE: Final[tuple[str, ...]] = tuple(_MINT_PROBE.new() for _ in range(4))


def plan_of(
    p_old: Mapping[str, Collection[str]],
    p_new: Iterable[Collection[str]],
) -> tuple[ReconcilePlan, CountingIdFactory]:
    """A plan and the factory that served it, so a test can also assert what was NOT minted."""
    factory = CountingIdFactory()
    return reconcile_plan(p_old, p_new, factory), factory


def assignment_of(plan: ReconcilePlan, record_key: str) -> str:
    """The entity `record_key` would be assigned to, asserting there is exactly one.

    D3 allows at most one assignment per record, and a plan that emitted two would
    otherwise show up as an arbitrary winner of a `MERGE INTO` rather than as a defect.
    """
    matches = [row.entity_id for row in plan.assignments if row.record_key == record_key]
    assert len(matches) == 1, f"expected exactly one assignment for {record_key!r}, got {matches}"
    return matches[0]


def details_of(plan: ReconcilePlan, event_type: str) -> list[tuple[str, dict[str, object]]]:
    """`(entity_id, details)` for every planned event of one type, in plan order."""
    return [(event.entity_id, dict(event.details)) for event in plan.events_of_type(event_type)]


def test_set_equal_partition_emits_nothing() -> None:
    """AC1: `P_new` set-equal to `P_old` produces no assignment, no transition, no event.

    INV-PERM's first clause, and the one every idempotence claim downstream rests on:
    ER-080's "zero events on unchanged re-run" and T-IDEM-1 both read this. The
    assertion is deliberately on the whole plan rather than on the event list alone,
    because a re-asserted membership row would move `assigned_at` — "the most recent
    (re)assignment" (S4.5.3) — for a record nothing happened to.
    """
    p_old = {E1: {"crm:1", "crm:2"}, E2: {"crm:3"}}
    plan, factory = plan_of(p_old, [{"crm:3"}, {"crm:2", "crm:1"}])

    assert plan.assignments == ()
    assert plan.transitions == ()
    assert plan.events == ()
    assert plan.minted == ()
    assert plan.is_empty
    # Zero mints, stated as the factory sees it: its next id is still the first one a
    # fresh factory would give.
    assert factory.new() == MINT_SEQUENCE[0]


def test_claimant_tiebreak_by_min_record_key_in_overlap() -> None:
    """AC2: equal overlaps are broken by the smallest `record_key` INSIDE the overlap.

    `E1` owns the smallest key in the fixture (`crm:1`), and a claimant rule that
    tiebroke on the entity's own minimum — or on `created_at`, or on the lexical
    `entity_id`, both of which S4.5.3 calls useless here — would hand the group to `E1`.
    The rule is scoped to the overlap, where `E2` opens at `crm:2` and `E1` only at
    `crm:5`, so `E2` claims it and `E1` is the loser.

    `E1` keeps a member outside the overlap on purpose: that is the only shape in which
    "inside the overlap" and "the entity's minimum" can disagree at all.
    """
    p_old = {E1: {"crm:1", "crm:5", "crm:6"}, E2: {"crm:2", "crm:3"}}
    plan, _ = plan_of(p_old, [{"crm:2", "crm:3", "crm:5", "crm:6"}, {"crm:1"}])

    assert assignment_of(plan, "crm:5") == E2
    assert assignment_of(plan, "crm:6") == E2
    assert details_of(plan, MERGED) == [
        (E1, {"member_keys": ["crm:5", "crm:6"], "merged_into": E2}),
    ]
    merged = [row for row in plan.transitions if row.status == MERGED]
    assert merged == [reconcile.EntityTransition(E1, MERGED, merged_into=E2)]
    assert plan.statuses[E2] == ACTIVE


def test_merge_and_split_at_once() -> None:
    """AC3: one group that is `E1 ∪ E2 ∪ a proper subset of E3`.

    The case S4.5.3 says a four-branch list keyed on "how many prior entities does this
    cluster touch" gets wrong. `E3` supplies the largest overlap so it claims the group
    and mints nothing; `E1` and `E2` are absorbed whole; and `E3`'s remaining member is a
    departing fragment that mints and carries the `split`.
    """
    p_old = {E1: {"crm:1"}, E2: {"crm:2"}, E3: {"crm:3", "crm:4", "crm:5", "crm:6"}}
    plan, _ = plan_of(p_old, [{"crm:1", "crm:2", "crm:3", "crm:4", "crm:5"}, {"crm:6"}])

    assert details_of(plan, MERGED) == [
        (E1, {"member_keys": ["crm:1"], "merged_into": E3}),
        (E2, {"member_keys": ["crm:2"], "merged_into": E3}),
    ]
    # Exactly one mint, and it is the departing fragment's — never the claimant's.
    assert plan.minted == (MINT_SEQUENCE[0],)
    assert details_of(plan, SPLIT) == [
        (MINT_SEQUENCE[0], {"member_keys": ["crm:6"], "split_from": E3}),
    ]
    assert assignment_of(plan, "crm:1") == E3
    assert assignment_of(plan, "crm:2") == E3
    assert assignment_of(plan, "crm:6") == MINT_SEQUENCE[0]
    # The claimant's own members never moved, so they are not re-asserted.
    assert [row.record_key for row in plan.assignments] == ["crm:1", "crm:2", "crm:6"]


def test_two_two_split_resolved_by_min_record_key() -> None:
    """AC4: a 2–2 split, where only the fragment order can decide.

    Both fragments have `member_count = 2`, so `(member_count DESC, ...)` is a tie and the
    entity-level tiebreaks are properties both fragments share. `min member record_key
    ASC` is the only total order left, and it gives the id to `{crm:1, crm:2}`.

    The groups are supplied in the *reverse* of that order to prove the result comes from
    the sort inside the function and not from the caller's iteration order.
    """
    plan, _ = plan_of(
        {E1: {"crm:1", "crm:2", "crm:3", "crm:4"}},
        [{"crm:3", "crm:4"}, {"crm:1", "crm:2"}],
    )

    assert plan.minted == (MINT_SEQUENCE[0],)
    assert details_of(plan, SPLIT) == [
        (MINT_SEQUENCE[0], {"member_keys": ["crm:3", "crm:4"], "split_from": E1}),
    ]
    assert assignment_of(plan, "crm:3") == MINT_SEQUENCE[0]
    assert assignment_of(plan, "crm:4") == MINT_SEQUENCE[0]
    # The rank-1 fragment retains the entity_id, so neither of its members is reassigned.
    assert [row.record_key for row in plan.assignments] == ["crm:3", "crm:4"]
    assert plan.statuses[E1] == ACTIVE


def test_empty_fragment_retires_entity() -> None:
    """AC5: an old entity holding zero members retires, with exactly one `retired` event.

    An entity every one of whose records was tombstoned reaches reconciliation holding
    nothing (S4.5.5) — `AffectedSet` carries it precisely so it can be retired — and it
    must not be silently dropped, or it stays `active` with no members forever. The other
    entity is set-equal to its group, so the whole plan is that one retirement.
    """
    plan, factory = plan_of({E1: {"crm:1", "crm:2"}, E2: set()}, [{"crm:1", "crm:2"}])

    assert details_of(plan, RETIRED) == [(E2, {"member_keys": []})]
    assert plan.transitions == (reconcile.EntityTransition(E2, RETIRED),)
    assert plan.assignments == ()
    assert plan.minted == ()
    assert factory.new() == MINT_SEQUENCE[0]


def test_record_leaving_all_clusters_becomes_singleton() -> None:
    """AC5: a record in no new group becomes its own entity — one mint, one `created`.

    S4.5.3 states it as a rule of the mapping, not as a case of it: the record is absent
    from the clustering output entirely, so it appears in no overlap cell and can never
    be handed back the id of the entity it left. Its former entity survives holding the
    rest, and records the departure as `member_removed`.
    """
    plan, _ = plan_of({E1: {"crm:1", "crm:2"}}, [{"crm:1"}])

    singleton = MINT_SEQUENCE[0]
    assert plan.minted == (singleton,)
    assert details_of(plan, CREATED) == [(singleton, {"member_keys": ["crm:2"]})]
    assert assignment_of(plan, "crm:2") == singleton
    assert plan.statuses[singleton] == ACTIVE
    assert details_of(plan, MEMBER_REMOVED) == [
        (E1, {"member_keys": ["crm:2"], "cause": RECLUSTER}),
    ]


def test_member_added_and_member_removed_events() -> None:
    """AC6: joining an existing entity emits `member_added`; leaving one, `member_removed`.

    Both types are in S5's vocabulary and the S4.6 touched-set formula reads both, so an
    implementation that recorded a joining record only as a membership row would leave
    that entity out of the golden rebuild. `member_added` is scoped to records that
    belonged to *no* prior entity: one that arrives from another entity is already
    accounted for by that entity's `merged` or by the fragment's `split`, and emitting
    both would fold the same record twice on replay.
    """
    joined, _ = plan_of({E1: {"crm:1", "crm:2"}}, [{"crm:1", "crm:2", "crm:9"}])
    assert details_of(joined, MEMBER_ADDED) == [(E1, {"member_keys": ["crm:9"]})]
    assert joined.events_of_type(MEMBER_REMOVED) == ()
    assert joined.minted == ()
    assert assignment_of(joined, "crm:9") == E1

    left, _ = plan_of({E1: {"crm:1", "crm:2", "crm:3"}}, [{"crm:1", "crm:2"}, {"crm:3"}])
    assert details_of(left, MEMBER_REMOVED) == [
        (E1, {"member_keys": ["crm:3"], "cause": RECLUSTER}),
    ]
    assert left.events_of_type(MEMBER_ADDED) == ()


def test_mint_order_is_ascending_min_record_key() -> None:
    """AC7: ids are handed out in ascending order of each new group's minimum member.

    S4.5.4 makes this an explicit `ORDER BY` rather than an incidental scan order,
    "which DuckDB does not guarantee" — and which a Python `set` does not guarantee
    either. The groups are supplied descending so an implementation that minted in
    iteration order fails here.

    `crm:4` is an orphan and is ordered in with the groups, not appended after them: it
    is a group of the output partition too, and its former entity retires.
    """
    plan, _ = plan_of({E1: {"crm:4"}}, [{"crm:9"}, {"crm:3", "crm:7"}, {"crm:5"}])

    assert plan.minted == MINT_SEQUENCE
    assert assignment_of(plan, "crm:3") == MINT_SEQUENCE[0]
    assert assignment_of(plan, "crm:7") == MINT_SEQUENCE[0]
    assert assignment_of(plan, "crm:4") == MINT_SEQUENCE[1]
    assert assignment_of(plan, "crm:5") == MINT_SEQUENCE[2]
    assert assignment_of(plan, "crm:9") == MINT_SEQUENCE[3]
    assert details_of(plan, RETIRED) == [(E1, {"member_keys": ["crm:4"]})]


def test_plan_is_independent_of_input_iteration_order() -> None:
    """AC7: two calls with identical inputs and fresh factories produce equal plans.

    D1 is about the cluster→`entity_id` MAP, so equality is asserted on the whole plan
    and not merely on the partition it induces. The second call receives the same
    partitions with every container's iteration order reversed, which is the only degree
    of freedom a caller has and the one D2 says must not show through.
    """
    p_old = {E1: {"crm:1"}, E2: {"crm:2"}, E3: {"crm:3", "crm:4", "crm:5", "crm:6"}}
    p_new = [{"crm:1", "crm:2", "crm:3", "crm:4"}, {"crm:5"}, {"crm:8"}]

    first, _ = plan_of(p_old, p_new)
    shuffled = {entity_id: p_old[entity_id] for entity_id in reversed(list(p_old))}
    second, _ = plan_of(shuffled, list(reversed(p_new)))

    assert first == second
    assert first.minted == second.minted


def test_unassigned_only_cluster_mints_one_entity_per_group() -> None:
    """S8.4's first case: a cluster of records no entity has ever held.

    "If a new group overlaps no old entity, it mints a new `entity_id`" (S4.5.3) — and
    the event is `created`, carrying the membership the entity is born with, because
    `created` is the only record that this id came into existence at all.
    """
    plan, _ = plan_of({}, [{"crm:1", "crm:2"}])

    born = MINT_SEQUENCE[0]
    assert plan.minted == (born,)
    assert details_of(plan, CREATED) == [(born, {"member_keys": ["crm:1", "crm:2"]})]
    assert plan.transitions == (reconcile.EntityTransition(born, ACTIVE, is_new=True),)
    assert [(row.record_key, row.entity_id) for row in plan.assignments] == [
        ("crm:1", born),
        ("crm:2", born),
    ]


def test_every_assignment_targets_an_active_entity() -> None:
    """AC8: no assignment ever references a `merged` or `retired` entity.

    The invariant T-INV-1 asserts against a real lake ("every `entity_membership.entity_id`
    has `entities.status='active'`"), pulled forward to the plan that produces those rows.
    The fixture is deliberately the messy one: a merge, a split, a retirement and an
    orphan in a single call.
    """
    p_old = {E1: {"crm:1", "crm:5", "crm:6"}, E2: {"crm:2", "crm:3"}, E3: {"crm:7"}}
    plan, _ = plan_of(p_old, [{"crm:2", "crm:3", "crm:5", "crm:6"}, {"crm:1"}])

    assert {row.status for row in plan.transitions} == {ACTIVE, MERGED, RETIRED}
    for row in plan.assignments:
        assert plan.statuses[row.entity_id] == ACTIVE, (
            f"{row.record_key!r} would be assigned to {row.entity_id!r}, whose planned "
            f"status is {plan.statuses[row.entity_id]!r}"
        )


def test_planned_event_details_hash_matches_the_event_log() -> None:
    """A planned event and the row ER-074 writes for it carry the same `details_hash`.

    The idempotency key of S4.5.4 is `(run_id, entity_id, event_type, details_hash)`. If
    the plan hashed a document the writer then normalised differently, the key would not
    identify the same event on both sides of the stage and "a re-run producing identical
    output writes zero events" would be unassertable.
    """
    p_old = {E1: {"crm:1"}, E2: {"crm:2"}, E3: {"crm:3", "crm:4", "crm:5", "crm:6"}}
    plan, _ = plan_of(p_old, [{"crm:1", "crm:2", "crm:3", "crm:4", "crm:5"}, {"crm:6"}, {"crm:8"}])

    log = EventLog("01JRUNRUNRUNRUNRUNRUNRUNRU", ids=CountingIdFactory(start=1000))
    for planned in plan.events:
        written = log.emit(planned.entity_id, planned.event_type, planned.details)
        assert written.details_hash == planned.details_hash
        assert dict(written.details) == dict(planned.details)
    # Every planned event is distinct under the idempotency key, so none collapsed.
    assert len(log) == len(plan.events)


def test_planned_event_types_are_the_spec_vocabulary() -> None:
    """Every type this module can emit is in S5's `entity_events` vocabulary.

    `edge_cut` is the one member it never emits: that is S4.4.2's partition-level cut and
    belongs to ER-076, which is why the list below is a subset rather than an equality.
    """
    assert set(reconcile.PLANNED_EVENT_ORDER) < frozenset(EVENT_TYPES)
    assert set(reconcile.PLANNED_EVENT_ORDER) == {
        CREATED,
        MEMBER_ADDED,
        MEMBER_REMOVED,
        MERGED,
        SPLIT,
        RETIRED,
    }


def test_planned_statuses_match_the_lake_registry() -> None:
    """The statuses spelled locally are exactly `entities.status`'s domain (S5).

    `reconcile.py` imports nothing from `src/er/lake`, so its three status constants are
    literals. This is the drift guard that independence would otherwise cost: a value
    added to or renamed in the registry fails here rather than in a dbt
    `accepted_values` test one milestone later.
    """
    assert PLANNED_STATUSES == frozenset(ENTITY_STATUSES)


def test_reconcile_module_imports_no_lake_and_no_ulid() -> None:
    """The module reaches neither `src/er/lake` nor the ULID library.

    Both halves of "no lake access, no clock, no ULID except through the injected
    `IdFactory`" (S4.5.4, D10) are properties of the import graph, so the import graph is
    what is checked — parsed rather than grepped, because the module's prose says the
    words `lake` and `ULID` in exactly the sentences that explain why it does not use
    them.
    """
    tree = ast.parse(Path(reconcile.__file__).read_text(encoding="utf-8"))
    imported: set[str] = set()
    for node in ast.walk(tree):
        if isinstance(node, ast.Import):
            imported.update(alias.name for alias in node.names)
        elif isinstance(node, ast.ImportFrom) and node.module is not None:
            imported.add(node.module)

    forbidden = sorted(
        name
        for name in imported
        if name == "ulid" or name.startswith(("ulid.", "er.lake", "duckdb"))
    )
    assert forbidden == [], (
        f"src/er/entities/reconcile.py imports {forbidden}; the reconciler is pure "
        f"(S4.5.4 D1/D2) and mints only through the injected IdFactory (D10)"
    )


def test_overlap_matrix_cells_are_the_intersections() -> None:
    """The matrix is keyed by each group's minimum member and holds only non-empty cells.

    S4.5.3 writes the cells as `|g_new ∩ g_old|`; they are carried as the intersections
    because both of the section's tiebreaks are stated over the members of the overlap
    and a cardinality cannot answer either.
    """
    matrix = overlap_matrix(
        {E1: {"crm:1", "crm:5"}, E2: {"crm:2"}},
        [{"crm:1", "crm:2"}, {"crm:5"}],
    )
    assert matrix == {
        "crm:1": {E1: frozenset({"crm:1"}), E2: frozenset({"crm:2"})},
        "crm:5": {E1: frozenset({"crm:5"})},
    }


def test_fragment_rank_is_count_descending_then_key_ascending() -> None:
    """`(member_count DESC, min member record_key ASC)`, as an ascending sort key.

    A size-0 fragment is not ranked: it retires its entity (S4.5.3), and the minimum the
    key needs does not exist.
    """
    assert fragment_rank({"crm:3", "crm:9"}) < fragment_rank({"crm:1"})
    assert fragment_rank({"crm:1", "crm:4"}) < fragment_rank({"crm:2", "crm:3"})
    with pytest.raises(ValueError, match="size-0 fragment"):
        fragment_rank(frozenset())


@pytest.mark.parametrize(
    ("p_old", "p_new", "match"),
    [
        ({E1: {"crm:1"}}, [{"crm:1"}, {"crm:1", "crm:2"}], "two clustering groups"),
        ({E1: {"crm:1"}, E2: {"crm:1"}}, [{"crm:1"}], "two entities"),
        ({}, [set()], "no members"),
    ],
)
def test_non_partition_inputs_raise(
    p_old: Mapping[str, Collection[str]],
    p_new: list[Collection[str]],
    match: str,
) -> None:
    """A non-partition input raises instead of producing a silently meaningless matrix.

    A record in two new groups would be assigned twice, and D3 allows exactly one row per
    record; the failure would otherwise surface as an arbitrary `MERGE INTO` winner, far
    from the input that caused it.
    """
    with pytest.raises(PartitionError, match=match):
        reconcile_plan(p_old, p_new, CountingIdFactory())
