"""Relational preparation for an initial partition with no prior lifecycle state.

Clustering and lifecycle planning use the same SQL implementation as subsequent
runs. Plans are staged locally before any lake write, with all entity IDs minted
before event IDs. Only bounded ID generation and exact event encoding enter Python.
"""

from __future__ import annotations

from collections.abc import Iterator
from contextlib import ExitStack, contextmanager, suppress
from dataclasses import dataclass
from uuid import uuid4

import duckdb

from er.entities.cluster import label_propagate_relations
from er.entities.events import Event, EventLog
from er.entities.ids import IdFactory
from er.entities.relational import event_stream, lifecycle_plan, scalar
from er.errors import PreconditionFailure
from er.lake.bulk import staged_rows
from er.lake.model import SCHEMA_QUALIFIER
from er.matching.edges import materialize_current_edges
from er.obs.profiling import span

_LAKE = SCHEMA_QUALIFIER


def initial_load_eligible(connection: duckdb.DuckDBPyConnection, run_id: str) -> bool:
    """Prove the full standardized corpus is exactly this run's affected live set.

    History, assertions and partial deliveries use the general affected-set path.
    In particular, an empty membership table alone does not authorize clustering
    records delivered by a different run.
    """
    for table in ("entities", "entity_membership", "entity_events", "assertions", "cut_edges"):
        if connection.execute(f"SELECT EXISTS (SELECT 1 FROM {_LAKE}.{table})").fetchone() == (
            True,
        ):
            return False
    if connection.execute(f"SELECT EXISTS (SELECT 1 FROM {_LAKE}.int_std_records)").fetchone() != (
        True,
    ):
        return False
    if connection.execute(
        f"SELECT EXISTS (SELECT 1 FROM {_LAKE}.raw_records WHERE is_deleted)"
    ).fetchone() == (True,):
        return False
    if connection.execute(
        f"SELECT EXISTS (SELECT 1 FROM {_LAKE}.review_queue WHERE resolved_at IS NOT NULL)"
    ).fetchone() == (True,):
        return False
    # Every live key must be in the run-scoped batch arm. No raw history may seed
    # an extra absent key through the change/deletion arms.
    missing = connection.execute(
        f"SELECT EXISTS (SELECT 1 FROM {_LAKE}.int_std_records s WHERE NOT EXISTS ("
        f"SELECT 1 FROM {_LAKE}.raw_records r JOIN {_LAKE}.ingest_batches b "
        "ON r.ingest_batch_id=b.ingest_batch_id WHERE b.run_id=? "
        "AND s.record_key=r.source_system || ':' || r.source_record_id))",
        [run_id],
    ).fetchone()
    extra = connection.execute(
        f"SELECT EXISTS (SELECT 1 FROM {_LAKE}.raw_records r WHERE NOT EXISTS ("
        f"SELECT 1 FROM {_LAKE}.int_std_records s "
        "WHERE s.record_key=r.source_system || ':' || r.source_record_id))"
    ).fetchone()
    return missing == (False,) and extra == (False,)


@dataclass(frozen=True, slots=True)
class InitialPlan:
    entities: str
    membership: str
    events: Iterator[Event]
    records: int
    edges: int
    clusters: int
    iterations: int


@contextmanager
def prepare_initial_plan(
    connection: duckdb.DuckDBPyConnection,
    *,
    run_id: str,
    model_version: str,
    tf_snapshot_id: str,
    auto_merge: float,
    max_iterations: int,
    ids: IdFactory,
    reason: str | None = None,
) -> Iterator[InitialPlan | None]:
    """Prepare local relations or decline when first-load assumptions do not hold."""
    if not initial_load_eligible(connection, run_id):
        yield None
        return
    EventLog(run_id, ids=ids, reason=reason)  # Validate the reason before doing any work.
    with ExitStack() as stack:

        def table(schema: tuple[tuple[str, str], ...]) -> str:
            return stack.enter_context(staged_rows(connection, schema, ()))

        edges = table((("rec_a_key", "VARCHAR"), ("rec_b_key", "VARCHAR")))
        view = f"er_initial_edges_{uuid4().hex}"
        materialize_current_edges(connection, model_version, tf_snapshot_id, name=view)
        try:
            invalid = connection.execute(
                f"SELECT rec_a_key, rec_b_key FROM {view} WHERE rec_a_key >= rec_b_key LIMIT 1"
            ).fetchone()
            if invalid is not None:
                raise PreconditionFailure(f"match_scores holds a non-canonical pair: {invalid!r}")
            absent = connection.execute(
                f"SELECT EXISTS (SELECT 1 FROM {view} WHERE "
                f"rec_a_key NOT IN (SELECT record_key FROM {_LAKE}.int_std_records) OR "
                f"rec_b_key NOT IN (SELECT record_key FROM {_LAKE}.int_std_records))"
            ).fetchone()
            if absent == (True,):
                yield None
                return
            connection.execute(
                f"INSERT INTO {edges} SELECT rec_a_key, rec_b_key FROM {view} "
                "WHERE match_probability >= ?",
                [auto_merge],
            )
        finally:
            with suppress(duckdb.Error):
                connection.execute(f"DROP VIEW {view}")
        edge_count = connection.execute(f"SELECT count(*) FROM {edges}").fetchone()
        assert edge_count is not None
        ordered = table((("record_key", "VARCHAR"), ("label", "VARCHAR"), ("position", "BIGINT")))
        with span("reconcile.initial_cluster", unit="records"):
            with label_propagate_relations(
                connection, f"{_LAKE}.int_std_records", edges, max_iterations=max_iterations
            ) as (labels, iterations):
                connection.execute(
                    f"INSERT INTO {ordered} SELECT record_key, label, "
                    "row_number() OVER (ORDER BY label, record_key) AS position "
                    f"FROM {labels} ORDER BY label, record_key"
                )
        old = table((("record_key", "VARCHAR"), ("entity_id", "VARCHAR")))
        old_entities = table((("entity_id", "VARCHAR"),))
        with span("reconcile.initial_plan", unit="entities"):
            with lifecycle_plan(connection, old, old_entities, ordered, ids) as plan:
                yield InitialPlan(
                    plan.entities,
                    plan.membership,
                    event_stream(connection, plan.events, run_id=run_id, ids=ids, reason=reason),
                    scalar(connection, f"SELECT count(*) FROM {ordered}"),
                    int(edge_count[0]),
                    plan.clusters,
                    iterations,
                )
