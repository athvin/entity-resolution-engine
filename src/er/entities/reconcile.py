"""INV-PERM as a pure function: the overlap matrix of S4.5.3, and nothing else.

:func:`reconcile_plan` takes the current membership partition, the clustering output
over the same nodes and an :class:`~er.entities.ids.IdFactory`, and returns a
:class:`ReconcilePlan` — the membership assignments, the `entities` transitions and
the `entity_events` documents a reconcile would write. It opens no connection, reads
no relation, takes no clock reading and mints no ULID except through the injected
factory, which is what makes the S4.5.4 determinism claim testable rather than
asserted (D1/D2, D10).

**One mapping, not four branches.** S4.5.3 is explicit that the overlap matrix
"subsumes the merge, split, extend and mint cases and — unlike a four-branch list
keyed on the number of distinct prior entities per cluster — it correctly covers a
cluster that is **simultaneously a merge of two entities and a split of a third**".
A branch list gets that cluster wrong in a way no amount of case analysis repairs,
so there is no branch list here: every outcome falls out of who ends up holding
which `entity_id`.

**Who ends up holding an `entity_id`.** S4.5.3 states two rules that can name two
different owners for one id, and reconciling them is the only real decision this
module makes:

* the **claimant** rule — "each new group's claimant is the old entity with the
  largest overlap, tiebroken by `min member record_key ASC` within the overlap";
* the **fragment ordering** — "fragments of a split old entity are ranked by
  `(member_count DESC, min member record_key ASC)`. Rank 1 retains the `entity_id`".

They disagree whenever a new group's largest overlap is with an old entity whose
own rank-1 fragment lies in a *different* group. This module resolves it as an
offer and an acceptance, which is the fixpoint of the two rules read together:

1. every old entity **offers** its `entity_id` to its rank-1 fragment's group — that
   is the fragment ordering, and it is what makes the id unavailable anywhere else;
2. every new group **accepts** the offer with the largest overlap, tiebroken by the
   minimum member `record_key` inside that overlap — that is the claimant rule,
   evaluated over the ids actually up for grabs at that group;
3. a group holding no offer mints, and every old entity whose offer was declined is
   merged into the entity that declined it.

The rejected alternative is to read the claimant rule over *all* overlapping old
entities and let a group mint whenever its claimant's id is already spoken for. It
is the more literal reading of a single sentence, and it destroys permanent ids for
nothing: given `E1 = {a, b, c}`, `E2 = {d}` and new groups `{a, b}` and `{c, d}`, it
mints a fresh id for `{c, d}` and merges `E2` — whose sole member never moved — into
it. G2 is the whole point of INV-PERM, so a reading that re-keys a settled entity to
satisfy a tiebreak loses more than it explains. Both readings satisfy every
acceptance criterion of this ticket; this one also satisfies G2.

**The set-equality fast path is a theorem, not a branch.** INV-PERM requires that a
`P_new` group set-equal to a `P_old` group retain every `entity_id`, mint nothing and
emit nothing — including no re-assertion of an unchanged assignment, or ER-080's
"zero events on unchanged re-run" and T-IDEM-1 both fail. A set-equal group is the
only fragment of its old entity (the partitions are disjoint, so nothing else can
overlap it), so it receives that entity's offer, is the only candidate, accepts, and
then produces no assignment because no member changed entity and no event because
nothing joined, left, merged or split. Writing it as an early return would be a
second implementation of the same conclusion, and the two could drift.

**Events.** Six of S5's seven types are reachable from here (`edge_cut` is S4.4.2's
and belongs to ER-076), and each covers a disjoint set of member movements so that
folding them reproduces `entity_membership` exactly (ER-080):

* `created` — every minted entity, carrying the membership it is born with. A
  departing fragment's new entity gets a `split` *as well*, naming where its members
  came from; the two are not redundant, because `created` is the only record that
  the `entity_id` itself came into existence.
* `member_added` — records that belonged to **no** old entity joining an entity that
  survives. S4.6's touched-set formula reads this type, so a record joining an
  existing entity must produce one.
* `member_removed` — members a surviving entity no longer holds, with
  `cause='recluster'`: a pure plan cannot tell a tombstone from a supersession from
  an ordinary re-clustering (S4.5.5), and the stage that can (ER-074) refines it.
* `merged` / `split` / `retired` — S4.5.3's three named transitions, one event each.

**Scope, deliberately.** Nothing here writes a row, allocates a `seq`, stamps an
`occurred_at` or resolves a redirect. `ReconcilePlan` records `merged_into` edges;
:func:`er.entities.ids.resolve` and its cycle guard own the reading of them, and
`entity_membership` — never `merged_into` — is how current membership is resolved
(S4.5.3, D3). The SQL that applies this plan in one snapshot is ER-074's; clustering
is ER-071/ER-072's; the never-cut is ER-076's.
"""

from __future__ import annotations

from collections.abc import Collection, Iterable, Mapping
from dataclasses import dataclass
from types import MappingProxyType
from typing import Any, Final

from er.entities.events import EVENT_DETAILS_SCHEMA, details_hash
from er.entities.ids import IdFactory

__all__ = [
    "ACTIVE",
    "CREATED",
    "MEMBER_ADDED",
    "MEMBER_REMOVED",
    "MERGED",
    "PLANNED_EVENT_ORDER",
    "PLANNED_STATUSES",
    "RECLUSTER",
    "RETIRED",
    "SPLIT",
    "EntityTransition",
    "MembershipAssignment",
    "PartitionError",
    "PlannedEvent",
    "ReconcilePlan",
    "fragment_rank",
    "overlap_matrix",
    "reconcile_plan",
]

#: `entities.status` (S5). Spelled here rather than imported from
#: `er.lake.model.ENTITY_STATUSES` because this module imports nothing from
#: `src/er/lake` — a reconciler that can reach the lake package is a reconciler a
#: later edit can give a connection. The unit tests assert the two agree, so the
#: independence costs no drift protection.
ACTIVE: Final = "active"
#: Also the `merged` event type: S5 gives the status and the event the same name
#: because they record the same fact from the two sides of `entities`.
MERGED: Final = "merged"
#: Likewise both a status and an event type.
RETIRED: Final = "retired"

CREATED: Final = "created"
MEMBER_ADDED: Final = "member_added"
MEMBER_REMOVED: Final = "member_removed"
SPLIT: Final = "split"

#: The statuses a plan may assign. `merged_into` is non-NULL exactly for `merged`
#: (S5), which :class:`EntityTransition` enforces.
PLANNED_STATUSES: Final[frozenset[str]] = frozenset({ACTIVE, MERGED, RETIRED})

#: `member_removed.cause` for a member the new partition simply placed elsewhere
#: (S4.5.5). The only cause a *pure* plan can justify: `tombstone` and
#: `supersession` are facts about `raw_records`, which this function never sees.
RECLUSTER: Final = "recluster"

#: The order planned events are returned in, and therefore the order ER-074 hands
#: them to :class:`~er.entities.events.EventLog`, which is `seq` order and so the
#: `(occurred_at, seq)` replay order. A total order is required by D1; this
#: particular one reads as the causal story — an entity exists, gains members, loses
#: members, then merges, splits or retires.
PLANNED_EVENT_ORDER: Final[tuple[str, ...]] = (
    CREATED,
    MEMBER_ADDED,
    MEMBER_REMOVED,
    MERGED,
    SPLIT,
    RETIRED,
)

_EVENT_RANK: Final[Mapping[str, int]] = MappingProxyType(
    {event_type: rank for rank, event_type in enumerate(PLANNED_EVENT_ORDER)}
)


class PartitionError(ValueError):
    """`P_old` or `P_new` is not a partition — its groups overlap, or one is empty.

    Both inputs are partitions by construction upstream (`entity_membership` holds
    one row per record per D3, and connected components are disjoint), so this is a
    contract violation rather than a data condition. It is checked anyway because the
    overlap matrix of a non-partition is silently meaningless: a record in two new
    groups would be assigned twice, and D3's "at most one assignment per record"
    would fail downstream where the cause is no longer visible.
    """


@dataclass(frozen=True, slots=True)
class MembershipAssignment:
    """One `entity_membership` row this plan would write (S4.5.3, D3).

    `entity_membership` is CURRENT STATE and is maintained by `MERGE INTO`, so the
    plan emits an assignment only for a record whose entity actually **changed**.
    Re-asserting an unchanged one would move its `assigned_at` — "the most recent
    (re)assignment" — for a record nothing happened to, and would break INV-PERM's
    requirement that a set-equal group produce nothing at all.

    `source_system` and `source_record_id` are not carried: they are
    :func:`~er.entities.ids.split_record_key` of ``record_key``, and materialising a
    second copy of D6's identity here would be a second place for it to disagree.
    """

    record_key: str
    entity_id: str


@dataclass(frozen=True, slots=True)
class EntityTransition:
    """One `entities` row this plan would insert or update.

    Emitted for every entity the plan **touches**: a minted one, one that merged, one
    that retired, and one that survives having gained or lost a member (its
    `updated_at`/`updated_run_id` move even though its status does not). An entity
    neither of whose members changed gets no transition, which is what makes
    :attr:`ReconcilePlan.is_empty` true for an unchanged re-run.

    Attributes:
        entity_id: the entity, minted or pre-existing.
        status: a member of :data:`PLANNED_STATUSES`.
        merged_into: the claimant that absorbed this entity. Non-``None`` **iff**
            ``status`` is ``merged``, exactly as S5 declares the column. It is a
            redirect for external id resolution only and is never a way to resolve
            current membership (S4.5.3, D3).
        is_new: whether this `entity_id` came from the injected factory during this
            plan. The flag distinguishes an INSERT from an UPDATE for ER-074 without
            it having to re-read `entities`.
    """

    entity_id: str
    status: str
    merged_into: str | None = None
    is_new: bool = False

    def __post_init__(self) -> None:
        if self.status not in PLANNED_STATUSES:
            raise ValueError(
                f"{self.status!r} is not an entity status; S5 allows "
                f"{', '.join(sorted(PLANNED_STATUSES))}"
            )
        if (self.merged_into is not None) != (self.status == MERGED):
            raise ValueError(
                f"merged_into is non-NULL iff status='merged' (S5); got "
                f"status={self.status!r}, merged_into={self.merged_into!r}"
            )
        if self.is_new and self.status != ACTIVE:
            raise ValueError(f"a minted entity is born {ACTIVE!r}, not {self.status!r}")


@dataclass(frozen=True, slots=True)
class PlannedEvent:
    """One `entity_events` document, minus the stamps only a run can supply.

    `event_id`, `seq`, `run_id` and `occurred_at` are deliberately absent: three of
    them are `VOLATILE_COLUMNS` members and all four belong to the writer (ER-074),
    which replays this event through :meth:`~er.entities.events.EventLog.emit`.

    ``details`` is normalised here to exactly what that method would normalise it to
    — record-key lists ascending — so :attr:`details_hash` computed on a plan and
    `entity_events.details_hash` computed at write time are the same digest. If they
    were not, the S4.5.4 idempotency key `(run_id, entity_id, event_type,
    details_hash)` would not identify the same event on both sides of the stage, and
    "a re-run producing identical output writes zero events" would be untestable.
    """

    entity_id: str
    event_type: str
    details: Mapping[str, Any]

    def __post_init__(self) -> None:
        schema = EVENT_DETAILS_SCHEMA.get(self.event_type)
        if schema is None:
            raise ValueError(
                f"{self.event_type!r} is not an event type; S5 allows "
                f"{', '.join(sorted(EVENT_DETAILS_SCHEMA))}"
            )
        supplied = frozenset(self.details)
        missing = sorted(frozenset(schema.required) - supplied)
        if missing:
            raise ValueError(f"{self.event_type} details: missing {', '.join(missing)}")
        unknown = sorted(supplied - schema.accepted)
        if unknown:
            raise ValueError(f"{self.event_type} details: unknown {', '.join(unknown)}")
        normalised: dict[str, Any] = dict(self.details)
        for key in schema.record_key_lists:
            normalised[key] = sorted(normalised[key])
        if schema.pair_keys is not None:
            a_key, b_key = schema.pair_keys
            if not normalised[a_key] < normalised[b_key]:
                raise ValueError(
                    f"{self.event_type} details: {a_key} < {b_key} is required on every pair "
                    f"(S5.0, D9); canonicalise through er.entities.ids.canonicalize_pair"
                )
        for key, vocabulary in schema.vocabularies.items():
            if key in normalised and normalised[key] not in vocabulary:
                raise ValueError(
                    f"{self.event_type} details: {key}={normalised[key]!r} is not one of "
                    f"{', '.join(sorted(vocabulary))}"
                )
        object.__setattr__(self, "details", MappingProxyType(normalised))

    @property
    def details_hash(self) -> str:
        """The S4.5.4 digest of :attr:`details`, from the one implementation of it."""
        return details_hash(self.details)


@dataclass(frozen=True, slots=True)
class ReconcilePlan:
    """Everything one reconcile would write, as data.

    Every collection is ordered by a total order so that two runs over identical
    inputs return **equal** plans (D1): assignments by `record_key`, transitions by
    `entity_id`, events by :data:`PLANNED_EVENT_ORDER` then `entity_id` then
    `details_hash`, and ``minted`` in the mint order S4.5.4 fixes.
    """

    assignments: tuple[MembershipAssignment, ...]
    transitions: tuple[EntityTransition, ...]
    events: tuple[PlannedEvent, ...]
    #: The ids taken from the factory, in the order they were taken — ascending by
    #: the minimum member `record_key` of the group each was minted for (S4.5.4).
    minted: tuple[str, ...]

    @property
    def is_empty(self) -> bool:
        """Whether the plan would write nothing: INV-PERM's set-equal case."""
        return not (self.assignments or self.transitions or self.events)

    @property
    def statuses(self) -> Mapping[str, str]:
        """`entity_id -> planned status`, for the entities this plan touches."""
        return MappingProxyType(
            {transition.entity_id: transition.status for transition in self.transitions}
        )

    def events_of_type(self, event_type: str) -> tuple[PlannedEvent, ...]:
        """The planned events of one type, in plan order."""
        return tuple(event for event in self.events if event.event_type == event_type)


def fragment_rank(members: Collection[str]) -> tuple[int, str]:
    """S4.5.3's fragment ordering as an **ascending** sort key.

    `(member_count DESC, min member record_key ASC)`, rendered as
    ``(-member_count, min(members))`` so that ``min``/``sorted`` yield rank 1 first.
    It is a total order over the fragments of one old entity because the fragments
    are disjoint and non-empty, so no two share a minimum — and a total order is
    required, because the entity-level tiebreaks (`created_at`, lexical `entity_id`)
    are properties every fragment of one entity shares and can never separate a 2–2
    split.

    The claimant rule uses this same key over a new group's overlaps — "largest
    overlap, tiebroken by `min member record_key ASC` **within the overlap**" is the
    same ordering read along the other axis of the matrix — which is why there is one
    function and not two.

    Raises:
        ValueError: ``members`` is empty. A size-0 fragment is not ranked; it retires
            its entity (S4.5.3), and ranking it would need a minimum that does not
            exist.
    """
    if not members:
        raise ValueError("a size-0 fragment is not ranked; it retires its entity (S4.5.3)")
    return (-len(members), min(members))


def _group_key(members: Collection[str]) -> str:
    """A new group's identity before it has one: its minimum member `record_key`.

    New groups arrive without ids — that is the whole problem reconciliation solves —
    but they need a stable handle to be sorted, indexed and reported by. The minimum
    member is the natural one: the groups are disjoint so it is unique, it is what
    Splink's `cluster_id` already is (S4.5.2), and it is the key S4.5.4 fixes the mint
    order on, so no second ordering has to be invented for that.
    """
    return min(members)


def _canonical_groups(p_new: Iterable[Collection[str]]) -> dict[str, frozenset[str]]:
    """`P_new` as `group key -> members`, validated as a partition."""
    groups: dict[str, frozenset[str]] = {}
    owner: dict[str, str] = {}
    for raw in p_new:
        members = frozenset(raw)
        if not members:
            raise PartitionError("a clustering output group holds no members")
        key = _group_key(members)
        for member in sorted(members):
            previous = owner.get(member)
            if previous is not None:
                raise PartitionError(
                    f"{member!r} is in two clustering groups ({previous!r} and {key!r}); "
                    f"P_new must be a partition"
                )
            owner[member] = key
        groups[key] = members
    return groups


def _canonical_old(p_old: Mapping[str, Collection[str]]) -> dict[str, frozenset[str]]:
    """`P_old` as `entity_id -> members`, validated as a partition.

    An entity holding **no** members is legal and is retained: an entity every one of
    whose records was tombstoned reaches reconciliation precisely so that it can be
    retired (S4.5.3, S4.5.5), and dropping it here would leave it `active` forever.
    """
    old: dict[str, frozenset[str]] = {}
    owner: dict[str, str] = {}
    for entity_id, raw in p_old.items():
        members = frozenset(raw)
        for member in sorted(members):
            previous = owner.get(member)
            if previous is not None:
                raise PartitionError(
                    f"{member!r} is a member of two entities ({previous!r} and "
                    f"{entity_id!r}); entity_membership holds one row per record (D3)"
                )
            owner[member] = entity_id
        old[entity_id] = members
    return old


def overlap_matrix(
    p_old: Mapping[str, Collection[str]],
    p_new: Iterable[Collection[str]],
) -> dict[str, dict[str, frozenset[str]]]:
    """S4.5.3's overlap matrix: `group key -> entity_id -> g_new ∩ g_old`.

    The cells are the **intersections themselves**, not the cardinalities S4.5.3
    writes as `|g_new ∩ g_old|`. A cardinality cannot answer either tiebreak the
    section then depends on — the claimant's "min member `record_key` ASC within the
    overlap" and the fragment order's minimum — so a matrix of counts would have to
    be joined back against the partitions at every comparison.

    Only non-empty cells are present: the matrix is sparse in practice (an affected
    subgraph touches a handful of entities) and an absent cell and a zero cell mean
    the same thing everywhere it is read.

    Args:
        p_old: the current membership partition, `entity_id -> members`.
        p_new: the clustering output over the same nodes, as groups of `record_key`.

    Returns:
        The matrix, keyed by each new group's minimum member `record_key`.

    Raises:
        PartitionError: either argument is not a partition.
    """
    old = _canonical_old(p_old)
    matrix: dict[str, dict[str, frozenset[str]]] = {}
    for key, members in _canonical_groups(p_new).items():
        cells = {
            entity_id: members & entity_members
            for entity_id, entity_members in old.items()
            if members & entity_members
        }
        matrix[key] = cells
    return matrix


def _offers(matrix: Mapping[str, Mapping[str, frozenset[str]]]) -> dict[str, str]:
    """`entity_id -> the group key its rank-1 fragment lies in`.

    Step 1 of the module docstring's fixpoint: an entity's id belongs to its rank-1
    fragment (S4.5.3), so that is the one group it can be offered to, and every other
    fragment of it is a departing fragment by construction.
    """
    fragments: dict[str, dict[str, frozenset[str]]] = {}
    for key, cells in matrix.items():
        for entity_id, overlap in cells.items():
            fragments.setdefault(entity_id, {})[key] = overlap
    return {
        entity_id: min(by_group, key=lambda key: fragment_rank(by_group[key]))
        for entity_id, by_group in fragments.items()
    }


def _acceptances(
    matrix: Mapping[str, Mapping[str, frozenset[str]]],
    offers: Mapping[str, str],
) -> dict[str, str]:
    """`group key -> the entity_id that group keeps`, for groups holding an offer.

    Step 2: S4.5.3's claimant rule — largest overlap, tiebroken by the minimum member
    `record_key` inside that overlap — evaluated with :func:`fragment_rank` over the
    offers a group received. A group with no offer is absent and mints.
    """
    offered: dict[str, list[str]] = {}
    for entity_id, key in offers.items():
        offered.setdefault(key, []).append(entity_id)
    return {
        key: min(candidates, key=lambda entity_id: fragment_rank(matrix[key][entity_id]))
        for key, candidates in offered.items()
    }


def reconcile_plan(
    p_old: Mapping[str, Collection[str]],
    p_new: Iterable[Collection[str]],
    id_factory: IdFactory,
) -> ReconcilePlan:
    """Apply INV-PERM to a partition pair and return what a reconcile would write.

    PURE. No connection, no clock, no `run_id`, no ULID except through
    ``id_factory``: given the same two partitions and a fresh factory the result is
    equal, byte for byte, modulo the ids the factory hands out (S4.5.4 D1/D2).

    Args:
        p_old: the current membership partition of the affected nodes,
            `entity_id -> record keys`. An entity holding zero members is legal and
            retires (S4.5.3); it is how an entity whose every record was tombstoned
            is reachable at all.
        p_new: the clustering output over the same nodes, as groups of `record_key`.
            The iteration order is irrelevant — every ordering this function depends
            on is an explicit sort — and a record in `p_old` that appears in no group
            becomes its own entity, which is S4.5.3's "a record leaving all clusters
            becomes a singleton entity".
        id_factory: the injected :class:`~er.entities.ids.IdFactory` (production:
            ULID; tests: a monotonic counter). Called once per new entity, in
            ascending order of that entity's minimum member `record_key` — an
            explicit sort, because S4.5.4 forbids an incidental scan order and
            DuckDB guarantees none.

    Returns:
        The :class:`ReconcilePlan`. :attr:`ReconcilePlan.is_empty` is true exactly
        when the two partitions are equal as set-partitions.

    Raises:
        PartitionError: either argument is not a partition.
    """
    old = _canonical_old(p_old)
    groups = _canonical_groups(p_new)
    matrix = overlap_matrix(old, groups.values())
    offers = _offers(matrix)
    accepted = _acceptances(matrix, offers)

    entity_of_record = {
        member: entity_id for entity_id, members in old.items() for member in members
    }
    clustered = frozenset[str]().union(*groups.values()) if groups else frozenset[str]()
    # A record of the prior partition that the clustering placed nowhere. It becomes
    # its own entity with a freshly minted id (S4.5.3) rather than keeping the id of
    # the entity it left, which is why orphans are held out of the overlap matrix:
    # folded in as singleton groups they would offer their old entity its id back.
    orphans = sorted(frozenset(entity_of_record) - clustered)

    # S4.5.4's mint order, as one explicit sort over everything that needs an id.
    # Orphan singletons are ordered in with the unclaimed groups because they are new
    # groups of the output partition too, and "ascending order of their minimum
    # member `record_key`" is a statement about the partition, not about a loop.
    mint_keys = sorted([key for key in groups if key not in accepted] + orphans)
    minted_ids: dict[str, str] = {key: id_factory.new() for key in mint_keys}

    assignments: list[MembershipAssignment] = []
    transitions: list[EntityTransition] = []
    events: list[PlannedEvent] = []

    for key in sorted(groups):
        members = groups[key]
        kept = accepted.get(key)
        entity_id = minted_ids[key] if kept is None else kept
        assignments.extend(
            MembershipAssignment(member, entity_id)
            for member in sorted(members)
            if entity_of_record.get(member) != entity_id
        )
        if kept is None:
            events.append(PlannedEvent(entity_id, CREATED, {"member_keys": sorted(members)}))
            transitions.append(EntityTransition(entity_id, ACTIVE, is_new=True))
        else:
            joined = sorted(member for member in members if member not in entity_of_record)
            if joined:
                events.append(PlannedEvent(entity_id, MEMBER_ADDED, {"member_keys": joined}))
            left = sorted(old[kept] - members)
            if left:
                events.append(
                    PlannedEvent(
                        entity_id, MEMBER_REMOVED, {"member_keys": left, "cause": RECLUSTER}
                    )
                )
            if old[kept] != members:
                transitions.append(EntityTransition(entity_id, ACTIVE))
        # Every overlapping entity whose rank-1 fragment lies elsewhere left a
        # departing fragment in this group: one `split` event on the entity now
        # holding it, naming the entity it broke off from (S4.5.3). An entity whose
        # rank-1 fragment IS this group either kept its id here or was merged into
        # the one that outbid it, and the entity loop below emits that.
        for entity, overlap in sorted(matrix[key].items()):
            if offers[entity] != key:
                events.append(
                    PlannedEvent(
                        entity_id, SPLIT, {"member_keys": sorted(overlap), "split_from": entity}
                    )
                )

    for entity_id in sorted(old):
        offered_to = offers.get(entity_id)
        if offered_to is None:
            # No fragment anywhere: the entity holds zero members after the mapping.
            # `member_keys` is the membership that departed, which is what makes the
            # retirement foldable rather than merely announced (S4.5.3).
            events.append(PlannedEvent(entity_id, RETIRED, {"member_keys": sorted(old[entity_id])}))
            transitions.append(EntityTransition(entity_id, RETIRED))
            continue
        # `key` is always an accepted group: this entity offered its id there, so the
        # group held at least one offer and took one.
        claimant = accepted[offered_to]
        if claimant == entity_id:
            continue
        events.append(
            PlannedEvent(
                entity_id,
                MERGED,
                {"member_keys": sorted(matrix[offered_to][entity_id]), "merged_into": claimant},
            )
        )
        transitions.append(EntityTransition(entity_id, MERGED, merged_into=claimant))

    for orphan in orphans:
        entity_id = minted_ids[orphan]
        assignments.append(MembershipAssignment(orphan, entity_id))
        events.append(PlannedEvent(entity_id, CREATED, {"member_keys": [orphan]}))
        transitions.append(EntityTransition(entity_id, ACTIVE, is_new=True))

    return ReconcilePlan(
        assignments=tuple(sorted(assignments, key=lambda row: row.record_key)),
        transitions=tuple(sorted(transitions, key=lambda row: row.entity_id)),
        events=tuple(
            sorted(
                events,
                key=lambda row: (_EVENT_RANK[row.event_type], row.entity_id, row.details_hash),
            )
        ),
        minted=tuple(minted_ids[key] for key in mint_keys),
    )
