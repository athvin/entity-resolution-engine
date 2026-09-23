"""Relation-based lifecycle planning with the pure planner as its parity oracle."""

from __future__ import annotations

import json
from collections.abc import Iterator
from contextlib import ExitStack, contextmanager
from dataclasses import dataclass
from typing import Any

import duckdb

from er.entities.events import Event, EventLog, encode_details
from er.entities.ids import IdFactory
from er.entities.reconcile import PLANNED_EVENT_ORDER
from er.lake.bulk import relation_pages, staged_ids, staged_query, staged_rows


@dataclass(frozen=True)
class RelationalPlan:
    entities: str
    membership: str
    events: str
    placement: str
    clusters: int
    created: int
    merged: int
    split: int
    retired: int
    added: int
    removed: int


def scalar(connection: duckdb.DuckDBPyConnection, sql: str) -> int:
    row = connection.execute(sql).fetchone()
    assert row is not None
    return int(row[0])


def event_stream(
    connection: duckdb.DuckDBPyConnection,
    relation: str,
    *,
    run_id: str,
    ids: IdFactory,
    reason: str | None,
) -> Iterator[Event]:
    """Retain the exact event encoder, with one page of events resident at a time."""
    EventLog(run_id, ids=ids, reason=reason)  # Validate even an empty event stream.
    offset = 0
    for page in relation_pages(connection, relation, "entity_id, event_type, details"):
        log = EventLog(run_id, ids=ids, reason=reason, seq_offset=offset)
        for entity_id, event_type, details in page:
            log.emit(str(entity_id), str(event_type), json.loads(str(details)))
        yield from log
        offset += len(log)


@contextmanager
def lifecycle_plan(
    connection: duckdb.DuckDBPyConnection,
    old: str,
    old_entities: str,
    labels: str,
    ids: IdFactory,
) -> Iterator[RelationalPlan]:
    """Preserve overlap offers, acceptances, orphan IDs and event ordering in SQL.

    ``old`` contains record_key/entity_id; ``old_entities`` retains empty entities;
    ``labels`` contains record_key/label. Only ID generation and canonical event
    serialization enter Python. Relations live until this context exits.
    """
    with ExitStack() as stack:

        def stage(sql: str) -> str:
            return stack.enter_context(staged_query(connection, sql))

        groups = stage(f"SELECT label, count(*) AS size FROM {labels} GROUP BY label")
        overlap = stage(
            f"SELECT n.label, o.entity_id, count(*) AS size, min(n.record_key) AS first_key "
            f"FROM {labels} n JOIN {old} o USING(record_key) GROUP BY n.label, o.entity_id"
        )
        offers = stage(
            f"SELECT * FROM {overlap} QUALIFY row_number() OVER "
            "(PARTITION BY entity_id ORDER BY size DESC, first_key) = 1"
        )
        accepted = stage(
            f"SELECT * FROM {offers} QUALIFY row_number() OVER "
            "(PARTITION BY label ORDER BY size DESC, first_key) = 1"
        )
        orphan = stage(
            f"SELECT record_key AS label FROM {old} ANTI JOIN {labels} USING(record_key)"
        )
        mint = stage(
            "SELECT label, row_number() OVER (ORDER BY label) AS position FROM ("
            f"SELECT label FROM {groups} ANTI JOIN {accepted} USING(label) "
            f"UNION ALL SELECT label FROM {orphan})"
        )
        created = scalar(connection, f"SELECT count(*) FROM {mint}")
        minted_ids = stack.enter_context(staged_ids(connection, created, ids))
        minted = stage(
            f"SELECT m.label, i.id AS entity_id FROM {mint} m JOIN {minted_ids} i USING(position)"
        )
        owners = stage(
            f"SELECT label, entity_id FROM {accepted} UNION ALL "
            f"SELECT label, entity_id FROM {minted}"
        )
        placement = stage(
            f"SELECT n.record_key, o.entity_id FROM {labels} n JOIN {owners} o USING(label) "
            f"UNION ALL SELECT r.label AS record_key, m.entity_id FROM {orphan} r "
            f"JOIN {minted} m USING(label)"
        )
        membership = stage(
            "SELECT split_part(p.record_key, ':', 1) AS source_system, "
            "substr(p.record_key, strpos(p.record_key, ':') + 1) AS source_record_id, "
            "p.record_key, p.entity_id "
            f"FROM {placement} p LEFT JOIN {old} o USING(record_key) "
            "WHERE o.entity_id IS DISTINCT FROM p.entity_id"
        )
        losing = stage(
            "SELECT e.entity_id, CASE WHEN o.label IS NULL THEN 'retired' ELSE 'merged' END "
            "AS status, a.entity_id AS merged_into "
            f"FROM {old_entities} e LEFT JOIN {offers} o USING(entity_id) "
            f"LEFT JOIN {accepted} a USING(label) "
            "WHERE o.label IS NULL OR a.entity_id <> e.entity_id"
        )
        entities = stage(
            f"SELECT entity_id, 'active'::VARCHAR AS status, NULL::VARCHAR AS merged_into "
            f"FROM {minted} UNION ALL "
            "SELECT a.entity_id, 'active', NULL "
            f"FROM {accepted} a JOIN {groups} g USING(label) "
            f"JOIN (SELECT entity_id, count(*) AS size FROM {old} GROUP BY entity_id) o "
            "ON o.entity_id = a.entity_id WHERE a.size <> g.size OR a.size <> o.size "
            f"UNION ALL SELECT entity_id, status, merged_into FROM {losing}"
        )
        # Event membership sets are grouped before encoding. All array orderings
        # match _validated_details; object byte encoding remains Python's job.
        raw_events = stage(
            "SELECT m.entity_id, 'created'::VARCHAR AS event_type, "
            "json_object('member_keys', list(p.record_key ORDER BY p.record_key)) AS details "
            f"FROM {minted} m JOIN {placement} p USING(entity_id) GROUP BY m.entity_id "
            "UNION ALL SELECT a.entity_id, 'member_added', "
            "json_object('member_keys', list(n.record_key ORDER BY n.record_key)) "
            f"FROM {accepted} a JOIN {labels} n USING(label) "
            f"ANTI JOIN {old} o USING(record_key) GROUP BY a.entity_id "
            "UNION ALL SELECT a.entity_id, 'member_removed', "
            "json_object('member_keys', list(o.record_key ORDER BY "
            "o.record_key), 'cause', 'recluster') "
            f"FROM {accepted} a JOIN {old} o ON a.entity_id = o.entity_id "
            f"WHERE NOT EXISTS (SELECT 1 FROM {labels} n WHERE n.label = a.label "
            "AND n.record_key = o.record_key) GROUP BY a.entity_id "
            "UNION ALL SELECT p.entity_id, 'split', "
            "json_object('member_keys', list(n.record_key ORDER BY n.record_key), "
            "'split_from', o.entity_id) "
            f"FROM {labels} n JOIN {old} o USING(record_key) "
            f"JOIN {offers} f ON f.entity_id = o.entity_id "
            f"JOIN {owners} p ON p.label = n.label WHERE f.label <> n.label "
            "GROUP BY p.entity_id, o.entity_id "
            "UNION ALL SELECT l.entity_id, 'retired', "
            "json_object('member_keys', coalesce(list(o.record_key ORDER BY o.record_key) "
            "FILTER (WHERE o.record_key IS NOT NULL), []::VARCHAR[])) "
            f"FROM {losing} l LEFT JOIN {old} o USING(entity_id) WHERE l.status = 'retired' "
            "GROUP BY l.entity_id "
            "UNION ALL SELECT l.entity_id, 'merged', "
            "json_object('member_keys', list(n.record_key ORDER BY n.record_key), "
            "'merged_into', l.merged_into) "
            f"FROM {losing} l JOIN {offers} f USING(entity_id) "
            f"JOIN {old} o USING(entity_id) JOIN {labels} n "
            "ON n.record_key = o.record_key AND n.label = f.label "
            "WHERE l.status = 'merged' GROUP BY l.entity_id, l.merged_into"
        )
        numbered = stage(f"SELECT *, row_number() OVER () AS position FROM {raw_events}")

        def encoded_events() -> Iterator[tuple[Any, ...]]:
            for page in relation_pages(connection, numbered, "entity_id, event_type, details"):
                for entity_id, event_type, text in page:
                    details = json.loads(str(text))
                    encoded, digest = encode_details(details)
                    yield entity_id, event_type, encoded, digest

        encoded = stack.enter_context(
            staged_rows(
                connection,
                (
                    ("entity_id", "VARCHAR"),
                    ("event_type", "VARCHAR"),
                    ("details", "JSON"),
                    ("sort_hash", "VARCHAR"),
                ),
                encoded_events(),
            )
        )
        ranks = " ".join(
            f"WHEN '{kind}' THEN {rank}" for rank, kind in enumerate(PLANNED_EVENT_ORDER)
        )
        events = stage(
            f"SELECT entity_id, event_type, details, row_number() OVER (ORDER BY "
            f"CASE event_type {ranks} END, entity_id, sort_hash) AS position FROM {encoded}"
        )
        counts = dict(
            connection.execute(
                f"SELECT event_type, count(*) FROM {events} GROUP BY event_type"
            ).fetchall()
        )
        yield RelationalPlan(
            entities,
            membership,
            events,
            placement,
            scalar(connection, f"SELECT count(*) FROM {groups}"),
            created,
            int(counts.get("merged", 0)),
            int(counts.get("split", 0)),
            int(counts.get("retired", 0)),
            int(counts.get("member_added", 0)),
            int(counts.get("member_removed", 0)),
        )
