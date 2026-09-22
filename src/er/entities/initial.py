"""Bounded preparation for an initial partition with no prior lifecycle state.

Clustering uses the shared SQL loop. Each batch of disjoint new groups goes through
`reconcile_plan({}, groups, ids)`: there is still only one lifecycle mapping. Plans
are staged locally before any lake write, with all entity IDs minted before event
IDs, as in the ordinary path. Python retains a batch and its largest component.
"""

from __future__ import annotations

import json
from collections.abc import Iterator
from contextlib import ExitStack, contextmanager, suppress
from dataclasses import dataclass, replace
from itertools import batched, groupby
from typing import Any
from uuid import uuid4

import duckdb

from er.entities.cluster import label_propagate_relations
from er.entities.events import Event, EventLog, canonical_details
from er.entities.ids import IdFactory
from er.entities.reconcile import reconcile_plan
from er.errors import PreconditionFailure
from er.lake.bulk import BATCH_ROWS, insert_batches, staged_rows
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


def _pages(
    connection: duckdb.DuckDBPyConnection, relation: str, columns: str
) -> Iterator[list[tuple[Any, ...]]]:
    """Bound each fetch even when its consumer issues writes on this connection."""
    offset = 0
    while rows := connection.execute(
        f"SELECT {columns} FROM {relation} WHERE position > ? AND position <= ? ORDER BY position",
        [offset, offset + BATCH_ROWS],
    ).fetchall():
        yield rows
        offset += BATCH_ROWS


def _groups(connection: duckdb.DuckDBPyConnection, relation: str) -> Iterator[list[str]]:
    rows = (row for page in _pages(connection, relation, "label, record_key") for row in page)
    for _label, members in groupby(rows, key=lambda row: row[0]):
        yield [str(row[1]) for row in members]


def _events(
    connection: duckdb.DuckDBPyConnection,
    relation: str,
    *,
    run_id: str,
    ids: IdFactory,
    reason: str | None,
) -> Iterator[Event]:
    offset = 0
    for page in _pages(connection, relation, "entity_id, event_type, details"):
        log = EventLog(run_id, ids=ids, reason=reason)
        for entity_id, event_type, details in page:
            log.emit(str(entity_id), str(event_type), json.loads(str(details)))
        # Each fresh entity has exactly one created event; groups are disjoint
        # across batches, so the per-batch logs cannot hide a cross-batch duplicate.
        assert len(log) == len(page)
        for event in log:
            yield replace(event, seq=offset + event.seq)
        offset += len(log)


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
        entities = table(
            (("entity_id", "VARCHAR"), ("status", "VARCHAR"), ("merged_into", "VARCHAR"))
        )
        membership = table(
            tuple(
                (name, "VARCHAR")
                for name in ("source_system", "source_record_id", "record_key", "entity_id")
            )
        )
        planned = table((("entity_id", "VARCHAR"), ("event_type", "VARCHAR"), ("details", "JSON")))
        clusters = 0
        records = 0
        with span("reconcile.initial_plan", unit="entities"):
            for groups in batched(_groups(connection, ordered), BATCH_ROWS):
                plan = reconcile_plan({}, groups, ids)
                clusters += len(plan.transitions)
                records += len(plan.assignments)
                insert_batches(
                    connection,
                    f"INSERT INTO {entities} SELECT unnest(?::VARCHAR[]), "
                    "unnest(?::VARCHAR[]), unnest(?::VARCHAR[])",
                    ((t.entity_id, t.status, t.merged_into) for t in plan.transitions),
                    columns=3,
                )
                insert_batches(
                    connection,
                    f"INSERT INTO {membership} SELECT unnest(?::VARCHAR[]), "
                    "unnest(?::VARCHAR[]), unnest(?::VARCHAR[]), unnest(?::VARCHAR[])",
                    (
                        (*a.record_key.split(":", 1), a.record_key, a.entity_id)
                        for a in plan.assignments
                    ),
                    columns=4,
                )
                insert_batches(
                    connection,
                    f"INSERT INTO {planned} SELECT unnest(?::VARCHAR[]), "
                    "unnest(?::VARCHAR[]), unnest(?::JSON[])",
                    (
                        (e.entity_id, e.event_type, canonical_details(e.details))
                        for e in plan.events
                    ),
                    columns=3,
                )
        events = table(
            (
                ("entity_id", "VARCHAR"),
                ("event_type", "VARCHAR"),
                ("details", "JSON"),
                ("position", "BIGINT"),
            )
        )
        # Every event is `created`, with a distinct entity ID. This is the same
        # event order as the complete plan, even for a non-monotonic test factory.
        connection.execute(
            f"INSERT INTO {events} SELECT *, row_number() OVER (ORDER BY entity_id) "
            f"AS position FROM {planned} ORDER BY entity_id"
        )
        yield InitialPlan(
            entities,
            membership,
            _events(connection, events, run_id=run_id, ids=ids, reason=reason),
            records,
            int(edge_count[0]),
            clusters,
            iterations,
        )
