"""`er reconcile`: the S4.5 chain, and the write that commits its plan (S4.5.3, D3).

ER-073 made the reconciler a pure function — given two partitions it returns the
membership assignments, entity transitions and events a reconcile *would* write. This
module is the half that runs it against a lake: it assembles the two partitions from
the affected set, calls the plan, and applies it.

**The order of the chain is normative, and two steps in it are ordering constraints
rather than data dependencies.**

1. `active_assertions`, then **CONTRADICTION-1** (S4.4.1). It runs *before* clustering
   and before anything is written, which is what makes the S4.7 guarantee — exit ``1``,
   no snapshot, no events, membership byte-identical — true by construction rather than
   by rollback. M6 is explicit that this is a hard deterministic failure and never a
   warning, so it is raised even when the affected set would have been empty: an
   unsatisfiable assertion set is a fact about the lake, not about this batch.
2. The affected node set (S4.5.1), then the affected edge set over it, then S4.4's
   assertion adjustment. `never` edges are removed and `always` edges injected at
   ``p = 1.0`` — **in memory only**. An assertion edge is never persisted to
   `match_scores`, which is what keeps `model_version` and `tf_snapshot_id` NOT NULL
   there and what makes `assertions` the durable record of a steward's decision (S4.4).
3. Label propagation to a bounded fixpoint (S4.5.2), then :func:`reconcile_plan`.
4. Apply.

**Membership is current state, written only by `MERGE INTO`** on
`(source_system, source_record_id)` — D3. Not delete+insert and not append: a record
has exactly one entity at any time, and the history lives in `entity_events`. A merge
loser's rows are rewritten to the survivor in the same statement as everyone else's, so
there is never an instant at which a record belongs to a `merged` entity. `merged_into`
is a redirect for external id resolution and is never a way to resolve current
membership.

**One snapshot per relation, not one per row.** DuckLake takes a snapshot per statement,
so all assignments go through one `MERGE`, all transitions through a second, and all
events through :func:`~er.entities.events.append_events`'s single insert. S4.5.3
requires an event and the membership rewrite it describes to be visible together; a
per-row flush would publish a half-written history.

**Nothing here re-derives the overlap mapping.** M5's whole point is that the mapping
lives in one place. This module reads :class:`~er.entities.reconcile.ReconcilePlan` and
writes what it says; a second implementation in SQL would be a second thing to keep
correct, and the two would disagree first on the case ER-073 exists for — a cluster that
is simultaneously a merge of two entities and a split of a third.
"""

from __future__ import annotations

import time
from collections.abc import Iterable, Mapping, Sequence
from dataclasses import dataclass
from datetime import UTC, datetime
from typing import Any, Final

import duckdb

from er.config.schema import Config
from er.entities.cluster import (
    adjust_edges_with_assertions,
    affected_edges,
    label_propagate,
    last_reconciled_watermark,
    load_affected_set,
)
from er.entities.events import EventLog, append_events
from er.entities.ids import IdFactory, MonotonicUlidFactory
from er.entities.reconcile import (
    MEMBER_ADDED,
    MEMBER_REMOVED,
    MERGED,
    RETIRED,
    SPLIT,
    ReconcilePlan,
    reconcile_plan,
)
from er.errors import ErrorClass, ExitCode, StageFailure
from er.lake.model import SCHEMA_QUALIFIER
from er.matching.edges import current_edges
from er.obs.runctx import StageRun
from er.review.assertions import Assertion, active_assertions, check_contradiction_1
from er.review.never_cut import never_cut_fixpoint, persist_cuts, release_cuts
from er.review.queue import upsert_escalation

__all__ = [
    "RECONCILE_STAGE",
    "ReconcileResult",
    "apply_reconcile_plan",
    "run_reconcile_stage",
]

#: The stage name S5.2 records this work under.
RECONCILE_STAGE: Final = "reconcile"

#: S5's event type for an S4.4.2 partition-level cut.
EDGE_CUT: Final = "edge_cut"

_MEMBERSHIP: Final = f"{SCHEMA_QUALIFIER}.entity_membership"
_ENTITIES: Final = f"{SCHEMA_QUALIFIER}.entities"
_EVENTS: Final = f"{SCHEMA_QUALIFIER}.entity_events"

#: `entity_membership` in S5 DDL order. Spelled from the registry nowhere here because
#: the MERGE names every column explicitly on both sides; a positional insert is what a
#: column addition breaks silently.
_MEMBERSHIP_COLUMNS: Final[tuple[str, ...]] = (
    "source_system",
    "source_record_id",
    "record_key",
    "entity_id",
    "assigned_at",
    "run_id",
)

_ENTITY_COLUMNS: Final[tuple[str, ...]] = (
    "entity_id",
    "status",
    "merged_into",
    "created_at",
    "updated_at",
    "created_run_id",
    "updated_run_id",
)


def _values_clause(rows: int, columns: int) -> str:
    """``(?, ?, …), (?, ?, …)`` for ``rows`` rows of ``columns`` placeholders each."""
    one = "(" + ", ".join("?" for _ in range(columns)) + ")"
    return ", ".join(one for _ in range(rows))


@dataclass(frozen=True)
class ReconcileResult:
    """What one reconcile did, in the terms S4.0 prints and S4.5.6 counts."""

    exit_code: int
    affected_entities: int
    affected_edges: int
    label_prop_iterations: int
    clusters_out: int
    entities_created: int
    entities_merged: int
    entities_split: int
    entities_retired: int
    members_added: int
    members_removed: int
    events_emitted: int
    edges_cut: int = 0
    cut_iterations: int = 0
    never_unsatisfiable_escalations: int = 0

    def manifest(self) -> dict[str, Any]:
        """The S4.0 stdout document for this stage."""
        return {
            "stage": RECONCILE_STAGE,
            "affected_entities": self.affected_entities,
            "affected_edges": self.affected_edges,
            "clusters_out": self.clusters_out,
            "entities_created": self.entities_created,
            "entities_merged": self.entities_merged,
            "entities_split": self.entities_split,
            "entities_retired": self.entities_retired,
            "events_emitted": self.events_emitted,
        }

    def stdout_line(self) -> str:
        """The human-readable one-liner S4.0 puts beside the manifest."""
        return (
            f"reconcile: {self.clusters_out} cluster(s), "
            f"+{self.entities_created} / merged {self.entities_merged} / "
            f"split {self.entities_split} / retired {self.entities_retired}, "
            f"{self.events_emitted} event(s)"
        )

    def record(self, run_ctx: StageRun, *, duration_ms: int) -> None:
        """Write every S4.5.6 counter onto ``run_ctx``.

        `set` routes each name to its promoted `run_stages` column where S5.2 declares
        one and to the JSON payload otherwise, so the four `entities_*` columns and the
        counters object stay consistent without this function knowing which is which.
        """
        counters = run_ctx.counters
        counters.set("affected_entities", self.affected_entities)
        counters.set("affected_edges", self.affected_edges)
        counters.set("label_prop_iterations", self.label_prop_iterations)
        counters.set("clusters_out", self.clusters_out)
        counters.set("entities_created", self.entities_created)
        counters.set("entities_merged", self.entities_merged)
        counters.set("entities_split", self.entities_split)
        counters.set("entities_retired", self.entities_retired)
        counters.set("members_added", self.members_added)
        counters.set("members_removed", self.members_removed)
        counters.set("events_emitted", self.events_emitted)
        counters.set("edges_cut", self.edges_cut)
        counters.set("cut_iterations", self.cut_iterations)
        counters.set("never_unsatisfiable_escalations", self.never_unsatisfiable_escalations)
        counters.set("duration_ms", duration_ms)


def _nothing_to_do() -> ReconcileResult:
    """S4.0's ``10``: an empty affected set is not a failure and writes nothing."""
    return ReconcileResult(
        exit_code=int(ExitCode.NOTHING_TO_DO),
        affected_entities=0,
        affected_edges=0,
        label_prop_iterations=0,
        clusters_out=0,
        entities_created=0,
        entities_merged=0,
        entities_split=0,
        entities_retired=0,
        members_added=0,
        members_removed=0,
        events_emitted=0,
    )


def _contradiction_failure(assertions: Sequence[Assertion]) -> StageFailure:
    """CONTRADICTION-1's hard failure, with the diagnosis S4.4.1 requires (M6).

    The message names every offending `assertion_id` and the always-closure component
    the `never` sits inside, because the operator's next action is to retract one of
    them and they cannot choose which without seeing the closure.
    """
    found = check_contradiction_1(assertions)
    ids = sorted({str(identifier) for row in found for identifier in row.assertion_ids})
    components = [sorted(row.component) for row in found]
    detail = "\n".join(
        [
            f"CONTRADICTION-1 (S4.4.1): {len(found)} never assertion(s) fall inside an "
            "always-closure component, so the assertion set is unsatisfiable and no "
            "clustering of it can honour every steward decision.",
            f"  assertion_ids: {ids}",
            *(f"  always-closure component: {component}" for component in components),
            "  Retract one of the named assertions and re-run; nothing was written.",
        ]
    )
    return StageFailure(
        f"CONTRADICTION-1: {len(found)} never assertion(s) inside an always closure",
        error_class=ErrorClass.CONTRADICTION,
        detail=detail,
    )


def _groups(labels: Mapping[str, str]) -> list[frozenset[str]]:
    """The label mapping as clustering output: one group per component."""
    grouped: dict[str, set[str]] = {}
    for key, label in labels.items():
        grouped.setdefault(label, set()).add(key)
    return [frozenset(members) for members in grouped.values()]


def _current_partition(
    connection: duckdb.DuckDBPyConnection, nodes: Iterable[str]
) -> dict[str, frozenset[str]]:
    """`entity_id -> members` for every entity holding one of ``nodes``.

    Read whole-entity rather than restricted to ``nodes``: S4.5.3's overlap matrix is
    between the *current* partition and the new one, and an entity seen with only some
    of its members would look like it had lost the rest — the plan would then emit
    `member_removed` events for records nothing touched.
    """
    rows = connection.execute(
        f"SELECT entity_id, record_key FROM {_MEMBERSHIP} WHERE entity_id IN ("
        f"SELECT DISTINCT entity_id FROM {_MEMBERSHIP} WHERE record_key IN ("
        + ", ".join("?" for _ in list(nodes))
        + "))",
        list(nodes),
    ).fetchall()
    grouped: dict[str, set[str]] = {}
    for entity_id, key in rows:
        grouped.setdefault(str(entity_id), set()).add(str(key))
    return {entity_id: frozenset(members) for entity_id, members in grouped.items()}


def apply_reconcile_plan(
    connection: duckdb.DuckDBPyConnection,
    plan: ReconcilePlan,
    *,
    run_id: str,
    occurred_at: datetime | None = None,
    ids: IdFactory | None = None,
    extra_events: Sequence[tuple[str, str, Mapping[str, Any]]] = (),
) -> int:
    """Commit ``plan``: membership, entities, events. Returns the events written.

    Three statements, one per relation, in an order that is itself a guarantee:
    `entities` first so no membership row can reference an entity that does not exist,
    then `entity_membership`, then the events describing what just changed.

    Args:
        connection: an attached lake connection.
        plan: the ER-073 plan. Nothing here re-derives it.
        run_id: stamped onto every row this writes.
        occurred_at: the events' stamp; now, in UTC, when omitted.
        ids: the `event_id` source, injected for determinism in tests (D10).
        extra_events: `(entity_id, event_type, details)` triples the plan cannot know
            about — S4.4.2's `edge_cut` is the only one today. They join the SAME log
            and therefore the same single append, because S4.5.3 requires an event and
            the membership change it describes to land in one snapshot; a second flush
            for the cuts would publish half the history.

    Returns:
        How many `entity_events` rows were appended.
    """
    stamp = datetime.now(UTC).replace(tzinfo=None) if occurred_at is None else occurred_at

    if plan.transitions:
        rows: list[Any] = []
        for transition in plan.transitions:
            rows += [
                transition.entity_id,
                transition.status,
                transition.merged_into,
                stamp,
                run_id,
            ]
        connection.execute(
            f"MERGE INTO {_ENTITIES} AS target USING (VALUES "
            f"{_values_clause(len(plan.transitions), 5)}"
            ") AS source(entity_id, status, merged_into, stamp, run_id) "
            "   ON target.entity_id = source.entity_id "
            " WHEN MATCHED THEN UPDATE SET status = source.status, "
            "        merged_into = source.merged_into, updated_at = source.stamp, "
            "        updated_run_id = source.run_id "
            f" WHEN NOT MATCHED THEN INSERT ({', '.join(_ENTITY_COLUMNS)}) "
            "      VALUES (source.entity_id, source.status, source.merged_into, "
            "              source.stamp, source.stamp, source.run_id, source.run_id)",
            rows,
        )

    if plan.assignments:
        member_rows: list[Any] = []
        for assignment in plan.assignments:
            source_system, source_record_id = assignment.record_key.split(":", 1)
            member_rows += [
                source_system,
                source_record_id,
                assignment.record_key,
                assignment.entity_id,
                stamp,
                run_id,
            ]
        connection.execute(
            f"MERGE INTO {_MEMBERSHIP} AS target USING (VALUES "
            f"{_values_clause(len(plan.assignments), 6)}"
            ") AS source(source_system, source_record_id, record_key, entity_id, "
            "            assigned_at, run_id) "
            "   ON target.source_system = source.source_system "
            "  AND target.source_record_id = source.source_record_id "
            " WHEN MATCHED THEN UPDATE SET entity_id = source.entity_id, "
            "        assigned_at = source.assigned_at, run_id = source.run_id "
            f" WHEN NOT MATCHED THEN INSERT ({', '.join(_MEMBERSHIP_COLUMNS)}) "
            "      VALUES (source.source_system, source.source_record_id, "
            "              source.record_key, source.entity_id, source.assigned_at, "
            "              source.run_id)",
            member_rows,
        )

    log = EventLog(run_id, ids=ids)
    for planned in plan.events:
        log.emit(planned.entity_id, planned.event_type, planned.details)
    for entity_id, event_type, details in extra_events:
        log.emit(entity_id, event_type, details)

    # `EventLog` collapses duplicates WITHIN one accumulation; this filters the ones
    # already committed by an earlier apply under the same `run_id`. Both halves are
    # needed and neither subsumes the other: the log makes a plan that reached the same
    # conclusion twice emit one row, and this makes re-applying a plan a no-op. S4.5.4
    # states the idempotency key as `(run_id, entity_id, event_type, details_hash)`, so
    # that tuple is what is compared — not the row count, which a retry would inflate.
    recorded = {
        (str(entity_id), str(event_type), str(digest))
        for entity_id, event_type, digest in connection.execute(
            f"SELECT entity_id, event_type, details_hash FROM {_EVENTS} WHERE run_id = ?",
            [run_id],
        ).fetchall()
    }
    fresh = [
        event
        for event in log
        if (event.entity_id, event.event_type, event.details_hash) not in recorded
    ]
    return append_events(connection, fresh, occurred_at=stamp)


def run_reconcile_stage(
    connection: duckdb.DuckDBPyConnection,
    cfg: Config,
    run_ctx: StageRun,
    *,
    model_version: str,
    tf_snapshot_id: str,
    id_factory: IdFactory | None = None,
    occurred_at: datetime | None = None,
) -> ReconcileResult:
    """Run the S4.5 chain and commit its plan.

    Args:
        connection: an open S4.0b connection with the lake attached.
        cfg: the validated S6 document; `thresholds` and `clustering` are read.
        run_ctx: this stage's `run_stages` row.
        model_version: the run's model version, which selects the edge set.
        tf_snapshot_id: the run's TF snapshot.
        id_factory: the source of minted `entity_id`s and `event_id`s (D10).
        occurred_at: the stamp every written row carries.

    Returns:
        The counters, with :attr:`ReconcileResult.exit_code` ``10`` for an empty
        affected set and ``0`` otherwise.

    Raises:
        er.errors.StageFailure: CONTRADICTION-1 holds (exit ``1``, `error_class`
            ``contradiction``), raised before clustering and before any write.
        er.entities.cluster.NonConvergenceError: label propagation exceeded
            `clustering.max_iterations` (exit ``1``), likewise before any write.
    """
    started = time.monotonic()
    factory: IdFactory = MonotonicUlidFactory() if id_factory is None else id_factory
    assertions = active_assertions(connection)

    # Before clustering and before any write (S4.4.1, M6). Deliberately ahead of the
    # affected-set query too: an unsatisfiable assertion set is a property of the lake,
    # so reporting "nothing to do" for it would hide the contradiction until a later
    # batch happened to be non-empty.
    if check_contradiction_1(assertions):
        raise _contradiction_failure(assertions)

    watermark = last_reconciled_watermark(connection)
    scored = current_edges(connection, model_version, tf_snapshot_id)
    affected = load_affected_set(
        connection,
        run_id=run_ctx.run_id,
        auto_merge=cfg.thresholds.auto_merge,
        edges=scored,
        assertions=assertions,
        watermark=watermark,
    )
    if not affected.nodes:
        return _nothing_to_do()

    nodes = tuple(sorted(affected.nodes))
    edges = affected_edges(
        connection,
        nodes,
        model_version=model_version,
        tf_snapshot_id=tf_snapshot_id,
        auto_merge=cfg.thresholds.auto_merge,
    )
    adjusted = adjust_edges_with_assertions(edges, assertions, nodes=nodes)

    propagation = label_propagate(
        connection,
        nodes,
        [edge.pair for edge in adjusted],
        max_iterations=cfg.clustering.max_iterations,
    )

    # S4.4.2 between clustering and the plan. A `never` is enforced at the PARTITION
    # level, so it needs a clustering to look at — and the plan must be built over the
    # POST-cut partition, or INV-PERM would be applied to an answer the cut is about to
    # change. Releasing first is what lets a retracted `never` re-merge in the same run
    # that retracted it: a stale active cut would keep the component apart for one more
    # run and the retraction would look like it had not taken.
    release_cuts(connection, run_id=run_ctx.run_id, released_at=occurred_at)
    cut = never_cut_fixpoint(
        [(edge.rec_a_key, edge.rec_b_key, edge.match_probability) for edge in adjusted],
        assertions,
        nodes=nodes,
        cut_protect_probability=cfg.clustering.cut_protect_probability,
        max_iterations=cfg.clustering.max_iterations,
    )
    if cut.cuts:
        adjusted = [edge for edge in adjusted if edge.pair not in cut.cut_pairs]
        propagation = label_propagate(
            connection,
            nodes,
            [edge.pair for edge in adjusted],
            max_iterations=cfg.clustering.max_iterations,
        )

    groups = _groups(propagation.labels)
    plan = reconcile_plan(_current_partition(connection, nodes), groups, factory)

    # S4.4.2 step 5: an `edge_cut` event on the affected entity. The id is minted here
    # rather than inside `persist_cuts` because the event carries it too — one mint,
    # two writers, so the row and the event name the same cut.
    cut_ids = {cut_edge.pair: factory.new() for cut_edge in cut.cuts}
    placement = {assignment.record_key: assignment.entity_id for assignment in plan.assignments}
    for entity_id, members in _current_partition(connection, nodes).items():
        for member in members:
            placement.setdefault(member, entity_id)
    cut_events = [
        (
            placement[cut_edge.rec_a_key],
            EDGE_CUT,
            {
                "rec_a_key": cut_edge.rec_a_key,
                "rec_b_key": cut_edge.rec_b_key,
                "match_probability": cut_edge.match_probability,
                "assertion_id": cut_edge.assertion_id,
                "cut_id": cut_ids[cut_edge.pair],
            },
        )
        for cut_edge in cut.cuts
        if cut_edge.rec_a_key in placement
    ]

    events_written = apply_reconcile_plan(
        connection,
        plan,
        run_id=run_ctx.run_id,
        occurred_at=occurred_at,
        ids=factory,
        extra_events=cut_events,
    )
    cuts_written = persist_cuts(
        connection,
        cut.cuts,
        run_id=run_ctx.run_id,
        model_version=model_version,
        tf_snapshot_id=tf_snapshot_id,
        cut_at=occurred_at,
        cut_ids=cut_ids,
    )
    # S4.4.2 step 4: a pair every path between which is protected is escalated rather
    # than cut. The assertion id travels in the CutResult for the operator's benefit but
    # is not a `review_queue` column (S5) — the row is keyed by the pair and the reason.
    for rec_a_key, rec_b_key, _assertion_id in cut.escalations:
        upsert_escalation(
            connection,
            rec_a_key=rec_a_key,
            rec_b_key=rec_b_key,
            run_id=run_ctx.run_id,
            id_factory=factory,
        )

    statuses = [transition.status for transition in plan.transitions]
    event_types = [planned.event_type for planned in plan.events]
    result = ReconcileResult(
        exit_code=int(ExitCode.SUCCESS),
        affected_entities=len(affected.entities),
        affected_edges=len(adjusted),
        label_prop_iterations=propagation.iterations,
        clusters_out=len(groups),
        entities_created=sum(1 for transition in plan.transitions if transition.is_new),
        entities_merged=statuses.count(MERGED),
        entities_split=event_types.count(SPLIT),
        entities_retired=statuses.count(RETIRED),
        members_added=event_types.count(MEMBER_ADDED),
        members_removed=event_types.count(MEMBER_REMOVED),
        events_emitted=events_written,
        edges_cut=cuts_written,
        cut_iterations=cut.iterations,
        never_unsatisfiable_escalations=len(cut.escalations),
    )
    result.record(run_ctx, duration_ms=int((time.monotonic() - started) * 1000))
    return result
