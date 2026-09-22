"""Orchestrate SQL reconciliation and preserve the pure planner's persistence API.

The production chain checks assertion contradictions and scoring generations before
writes, discovers affected records in DuckDB, clusters local relations, applies
ordered graph cuts when needed, and plans entity lifecycle changes relationally.
Only exact event encoding and ID generation cross the normal Python boundary.

`apply_reconcile_plan` remains available for collection callers and parity tests.
Both paths share the same entity/membership MERGEs and event append, preserving
current membership, redirect history, immutable event hashes and retry semantics.
"""

from __future__ import annotations

import time
from collections.abc import Iterable, Mapping, Sequence
from dataclasses import dataclass
from datetime import UTC, datetime
from typing import Any, Final

import duckdb

from er.config.schema import Config
from er.entities.events import EventLog, append_events
from er.entities.guards import assert_scoring_generation
from er.entities.ids import IdFactory, MonotonicUlidFactory
from er.entities.reconcile import (
    ReconcilePlan,
)
from er.errors import ErrorClass, ExitCode, StageFailure
from er.lake.bulk import staged_rows
from er.lake.model import SCHEMA_QUALIFIER
from er.obs.profiling import profiled
from er.obs.runctx import StageRun
from er.review.assertions import Assertion, active_assertions, check_contradiction_1

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

#: `member_removed.cause` for a record S4.1.1 deleted — one of S5's closed
#: `MEMBER_REMOVED_CAUSES`. The plan's own removals carry `recluster`; this one is
#: stamped by the stage because only the stage knows the record left the corpus
#: rather than merely leaving its cluster (S4.5.5).
TOMBSTONE_CAUSE: Final = "tombstone"

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


@profiled("reconcile.current_partition", "entities")
def _current_partition(
    connection: duckdb.DuckDBPyConnection, nodes: Iterable[str]
) -> dict[str, frozenset[str]]:
    """`entity_id -> members` for every entity holding one of ``nodes``.

    Read whole-entity rather than restricted to ``nodes``: S4.5.3's overlap matrix is
    between the *current* partition and the new one, and an entity seen with only some
    of its members would look like it had lost the rest — the plan would then emit
    `member_removed` events for records nothing touched.
    """
    requested = sorted(set(nodes))
    if not requested:
        return {}
    rows = connection.execute(
        f"SELECT entity_id, record_key FROM {_MEMBERSHIP} WHERE entity_id IN ("
        f"SELECT DISTINCT entity_id FROM {_MEMBERSHIP} "
        "WHERE record_key IN (SELECT unnest(?::VARCHAR[])))",
        [requested],
    ).fetchall()
    grouped: dict[str, set[str]] = {}
    for entity_id, key in rows:
        grouped.setdefault(str(entity_id), set()).add(str(key))
    return {entity_id: frozenset(members) for entity_id, members in grouped.items()}


def _merge_entity_relation(
    connection: duckdb.DuckDBPyConnection, staged: str, stamp: datetime, run_id: str
) -> None:
    """Commit a prepared local relation using the shared lifecycle write."""
    connection.execute(
        f"MERGE INTO {_ENTITIES} AS target USING "
        f"(SELECT *, CAST(? AS TIMESTAMP) AS stamp, ? AS run_id FROM {staged}) "
        "AS source ON target.entity_id = source.entity_id "
        " WHEN MATCHED THEN UPDATE SET status = source.status, "
        "        merged_into = source.merged_into, updated_at = source.stamp, "
        "        updated_run_id = source.run_id "
        f" WHEN NOT MATCHED THEN INSERT ({', '.join(_ENTITY_COLUMNS)}) "
        "      VALUES (source.entity_id, source.status, source.merged_into, "
        "              source.stamp, source.stamp, source.run_id, source.run_id)",
        [stamp, run_id],
    )


def _merge_membership_relation(
    connection: duckdb.DuckDBPyConnection, staged: str, stamp: datetime, run_id: str
) -> None:
    """Commit a prepared local relation using the shared lifecycle write."""
    connection.execute(
        f"MERGE INTO {_MEMBERSHIP} AS target USING "
        f"(SELECT *, CAST(? AS TIMESTAMP) AS assigned_at, ? AS run_id FROM {staged}) "
        "AS source ON target.source_system = source.source_system "
        "  AND target.source_record_id = source.source_record_id "
        " WHEN MATCHED THEN UPDATE SET entity_id = source.entity_id, "
        "        assigned_at = source.assigned_at, run_id = source.run_id "
        f" WHEN NOT MATCHED THEN INSERT ({', '.join(_MEMBERSHIP_COLUMNS)}) "
        "      VALUES (source.source_system, source.source_record_id, "
        "              source.record_key, source.entity_id, source.assigned_at, "
        "              source.run_id)",
        [stamp, run_id],
    )


@profiled("reconcile.persist", "entities")
def apply_reconcile_plan(
    connection: duckdb.DuckDBPyConnection,
    plan: ReconcilePlan,
    *,
    run_id: str,
    occurred_at: datetime | None = None,
    ids: IdFactory | None = None,
    extra_events: Sequence[tuple[str, str, Mapping[str, Any]]] = (),
    reason: str | None = None,
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
        with staged_rows(
            connection,
            (("entity_id", "VARCHAR"), ("status", "VARCHAR"), ("merged_into", "VARCHAR")),
            (
                (transition.entity_id, transition.status, transition.merged_into)
                for transition in plan.transitions
            ),
        ) as staged:
            _merge_entity_relation(connection, staged, stamp, run_id)

    if plan.assignments:
        with staged_rows(
            connection,
            tuple((column, "VARCHAR") for column in _MEMBERSHIP_COLUMNS[:4]),
            (
                (*assignment.record_key.split(":", 1), assignment.record_key, assignment.entity_id)
                for assignment in plan.assignments
            ),
        ) as staged:
            _merge_membership_relation(connection, staged, stamp, run_id)

    # A rebuild's reason rides every event of the run (S4.0, S5.1). It joins the
    # canonicalised details and therefore `details_hash`, so a stamped run's events
    # are new idempotency keys rather than duplicates of an ordinary run's.
    log = EventLog(run_id, ids=ids, reason=reason)
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
    return append_events(connection, log, occurred_at=stamp)


@profiled("reconcile.lifecycle", "entities")
def run_reconcile_stage(
    connection: duckdb.DuckDBPyConnection,
    cfg: Config,
    run_ctx: StageRun,
    *,
    model_version: str,
    tf_snapshot_id: str,
    id_factory: IdFactory | None = None,
    occurred_at: datetime | None = None,
    reason: str | None = None,
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

    # The S4.3.2 activation guard, likewise before anything is written: a current
    # edge set speaking two scoring generations above `review_low` is two
    # probability scales against one threshold, and the refusal (exit 3,
    # `precondition`) is what forces the full rescore first. Assertion edges can
    # never appear here — S4.4 keeps them out of `match_scores` entirely.
    assert_scoring_generation(
        connection,
        review_low=cfg.thresholds.review_low,
    )

    if not assertions:
        from er.entities.initial import prepare_initial_plan

        with prepare_initial_plan(
            connection,
            run_id=run_ctx.run_id,
            model_version=model_version,
            tf_snapshot_id=tf_snapshot_id,
            auto_merge=cfg.thresholds.auto_merge,
            max_iterations=cfg.clustering.max_iterations,
            ids=factory,
            reason=reason,
        ) as initial:
            if initial is not None:
                stamp = (
                    datetime.now(UTC).replace(tzinfo=None) if occurred_at is None else occurred_at
                )
                _merge_entity_relation(connection, initial.entities, stamp, run_ctx.run_id)
                _merge_membership_relation(connection, initial.membership, stamp, run_ctx.run_id)
                events_written = append_events(connection, initial.events, occurred_at=stamp)
                result = ReconcileResult(
                    exit_code=int(ExitCode.SUCCESS),
                    affected_entities=0,
                    affected_edges=initial.edges,
                    label_prop_iterations=initial.iterations,
                    clusters_out=initial.clusters,
                    entities_created=initial.clusters,
                    entities_merged=0,
                    entities_split=0,
                    entities_retired=0,
                    members_added=0,
                    members_removed=0,
                    events_emitted=events_written,
                )
                run_ctx.counters.set("rows_in", initial.records)
                run_ctx.counters.set("rows_out", initial.clusters)
                run_ctx.counters.set("input_unit", "records")
                run_ctx.counters.set("output_unit", "entities")
                result.record(run_ctx, duration_ms=int((time.monotonic() - started) * 1000))
                return result

    from er.entities.relational_stage import run_affected_reconcile

    return run_affected_reconcile(
        connection,
        cfg,
        run_ctx,
        model_version=model_version,
        tf_snapshot_id=tf_snapshot_id,
        assertions=assertions,
        ids=factory,
        occurred_at=occurred_at,
        reason=reason,
        started=started,
    )
