"""The tenant read path: DuckLake reads with snapshot-pinned keyset pagination.

docs/backend-design.md §8: reads come straight off read-only DuckDB attaches —
DuckLake MVCC means a running writer never blocks them — and pagination pins
the snapshot it started on (``AT (VERSION => n)``), so a run committing between
pages cannot shear a result set. The tenant's lake environment is supplied per
request through :func:`er.lake.env.lake_environment`; nothing here mutates
process env or opens a writer lock.

Every query is against relations the engine's registry declares
(`src/er/lake/model.py`); no user input reaches SQL as anything but a bound
parameter, and the snapshot is validated as an int before it is rendered.
"""

from __future__ import annotations

import base64
import json
import threading
import time
from collections.abc import Iterator, Mapping
from contextlib import ExitStack, contextmanager
from dataclasses import dataclass, field
from typing import Any

import duckdb

from er.lake.ducklake import connect, current_snapshot
from er.lake.env import lake_environment
from er.lake.model import SCHEMA_QUALIFIER

__all__ = [
    "close_pool",
    "decode_cursor",
    "duplicate_groups",
    "encode_cursor",
    "entity_detail",
    "golden_list",
    "merge_plans",
    "metrics",
    "open_lake",
    "runs_list",
]

#: Warm attaches kept per process; the design's §8 LRU. Verified safe for
#: freshness: DuckLake resolves the snapshot from the catalog at query time,
#: so a held attach sees commits made by other writers.
_POOL_CAPACITY = 8


@dataclass
class _PooledLake:
    key: tuple[tuple[str, str], ...]
    stack: ExitStack
    connection: duckdb.DuckDBPyConnection
    guard: threading.Lock = field(default_factory=threading.Lock)
    last_used: float = 0.0


_POOL: list[_PooledLake] = []
_POOL_LOCK = threading.Lock()


def _fresh(env: Mapping[str, str], key: tuple[tuple[str, str], ...]) -> _PooledLake:
    """Attach a new tenant connection.

    The env overlay wraps only the attach itself: :func:`connect` reads every
    variable while entering, and a contextvar token must be reset on the thread
    that set it — so the overlay never lives inside the pooled entry, which a
    different request thread may eventually close.
    """
    stack = ExitStack()
    with lake_environment(dict(env)):
        connection = stack.enter_context(connect())
    return _PooledLake(key=key, stack=stack, connection=connection)


def _discard(entry: _PooledLake) -> None:
    try:
        entry.stack.close()
    except Exception:  # noqa: BLE001 - teardown of a possibly-broken attach
        pass


@contextmanager
def open_lake(env: Mapping[str, str]) -> Iterator[duckdb.DuckDBPyConnection]:
    """A read connection to the tenant's lake, reused across requests.

    One tenant may hold several pooled attaches under concurrent requests
    (DuckDB connections are not safe for concurrent statements); a connection
    whose request raised is discarded rather than reused, so a broken attach
    cannot poison later reads. Beyond capacity, the least-recently-used idle
    entry is closed.
    """
    key = tuple(sorted(dict(env).items()))
    entry: _PooledLake | None = None
    with _POOL_LOCK:
        for candidate in _POOL:
            if candidate.key == key and candidate.guard.acquire(blocking=False):
                entry = candidate
                break
    if entry is None:
        entry = _fresh(env, key)
        entry.guard.acquire()
    try:
        yield entry.connection
    except Exception:
        with _POOL_LOCK:
            if entry in _POOL:
                _POOL.remove(entry)
        _discard(entry)
        raise
    entry.last_used = time.monotonic()
    with _POOL_LOCK:
        if entry not in _POOL:
            _POOL.append(entry)
        victims: list[_PooledLake] = []
        while len(_POOL) > _POOL_CAPACITY:
            evictable = [
                candidate
                for candidate in sorted(_POOL, key=lambda pooled: pooled.last_used)
                if candidate is not entry and candidate.guard.acquire(blocking=False)
            ]
            if not evictable:
                break
            victim = evictable[0]
            for spared in evictable[1:]:
                spared.guard.release()
            _POOL.remove(victim)
            victims.append(victim)
    for victim in victims:
        _discard(victim)
    entry.guard.release()


def close_pool() -> None:
    """Close every idle pooled attach; for tests and orderly shutdown."""
    with _POOL_LOCK:
        drained = [entry for entry in _POOL if entry.guard.acquire(blocking=False)]
        for entry in drained:
            _POOL.remove(entry)
    for entry in drained:
        _discard(entry)


def encode_cursor(snapshot: int, last_key: str) -> str:
    raw = json.dumps({"s": snapshot, "k": last_key}, separators=(",", ":"))
    return base64.urlsafe_b64encode(raw.encode()).decode()


def decode_cursor(cursor: str) -> tuple[int, str]:
    """``(snapshot, last_key)`` — raises ``ValueError`` on anything malformed."""
    try:
        payload = json.loads(base64.urlsafe_b64decode(cursor.encode()).decode())
        return int(payload["s"]), str(payload["k"])
    except Exception as exc:  # noqa: BLE001 - any malformation is the same 422
        raise ValueError("invalid cursor") from exc


def _at(table: str, snapshot: int, alias: str | None = None) -> str:
    """A pinned table reference; ``snapshot`` has been through ``int()``.

    DuckLake's grammar puts the alias before the time-travel clause:
    ``FROM t AS x AT (VERSION => n)``.
    """
    reference = f"{SCHEMA_QUALIFIER}.{table}"
    if alias is not None:
        reference += f" AS {alias}"
    return f"{reference} AT (VERSION => {int(snapshot)})"


_GOLDEN_COLUMNS = (
    "entity_id, given_name, family_name, email, phone_e164, addr_number, addr_street, "
    "addr_unit, addr_city, addr_region, addr_postal, birth_date, metadata, "
    "survivorship_version, assembled_at"
)


def _rows(result: duckdb.DuckDBPyConnection, sql: str, params: list[Any]) -> list[dict[str, Any]]:
    cursor = result.execute(sql, params)
    names = [description[0] for description in cursor.description]
    return [dict(zip(names, row, strict=True)) for row in cursor.fetchall()]


def golden_list(
    connection: duckdb.DuckDBPyConnection,
    *,
    snapshot: int | None = None,
    q: str | None = None,
    after_key: str | None = None,
    limit: int = 50,
) -> tuple[list[dict[str, Any]], int, str | None]:
    """One page of golden records: ``(rows, snapshot, next_cursor)``."""
    snap = snapshot if snapshot is not None else current_snapshot(connection)
    clauses: list[str] = []
    params: list[Any] = []
    if q:
        # A search term is literal text: escape LIKE's metacharacters so a
        # caller's `%`/`_` match themselves rather than everything.
        escaped = q.replace("\\", "\\\\").replace("%", "\\%").replace("_", "\\_")
        like = f"%{escaped}%"
        clauses.append(
            "(given_name ILIKE ? ESCAPE '\\' OR family_name ILIKE ? ESCAPE '\\' "
            "OR email ILIKE ? ESCAPE '\\')"
        )
        params += [like, like, like]
    if after_key is not None:
        clauses.append("entity_id > ?")
        params.append(after_key)
    where = f"WHERE {' AND '.join(clauses)}" if clauses else ""
    sql = (
        f"SELECT {_GOLDEN_COLUMNS} FROM {_at('golden_records', snap)} {where} "
        f"ORDER BY entity_id LIMIT {int(limit) + 1}"
    )
    rows = _rows(connection, sql, params)
    next_cursor = None
    if len(rows) > limit:
        rows = rows[:limit]
        next_cursor = encode_cursor(snap, str(rows[-1]["entity_id"]))
    return rows, snap, next_cursor


def entity_detail(connection: duckdb.DuckDBPyConnection, entity_id: str) -> dict[str, Any] | None:
    """Golden values plus members, per-attribute lineage, and event history."""
    golden = _rows(
        connection,
        f"SELECT {_GOLDEN_COLUMNS} FROM {SCHEMA_QUALIFIER}.golden_records WHERE entity_id = ?",
        [entity_id],
    )
    if not golden:
        return None
    members = _rows(
        connection,
        f"SELECT source_system, source_record_id, record_key, assigned_at, run_id "
        f"FROM {SCHEMA_QUALIFIER}.entity_membership WHERE entity_id = ? ORDER BY record_key",
        [entity_id],
    )
    lineage = _rows(
        connection,
        f"SELECT attribute, record_key, source_system, source_record_id, rule "
        f"FROM {SCHEMA_QUALIFIER}.golden_lineage WHERE entity_id = ? ORDER BY attribute",
        [entity_id],
    )
    events = _rows(
        connection,
        f"SELECT seq, run_id, event_type, details, occurred_at "
        f"FROM {SCHEMA_QUALIFIER}.entity_events WHERE entity_id = ? ORDER BY seq",
        [entity_id],
    )
    return {"golden": golden[0], "members": members, "lineage": lineage, "events": events}


def duplicate_groups(
    connection: duckdb.DuckDBPyConnection,
    *,
    snapshot: int | None = None,
    after_key: str | None = None,
    limit: int = 50,
) -> tuple[list[dict[str, Any]], int, str | None]:
    """Entities holding more than one source record — the merge grid's rows."""
    snap = snapshot if snapshot is not None else current_snapshot(connection)
    params: list[Any] = []
    after = ""
    if after_key is not None:
        after = "AND m.entity_id > ?"
        params.append(after_key)
    sql = f"""
        SELECT m.entity_id, count(*) AS member_count,
               list(m.source_system || ':' || m.source_record_id ORDER BY m.record_key)
                 AS members,
               any_value(g.given_name) AS given_name,
               any_value(g.family_name) AS family_name,
               any_value(g.email) AS email
        FROM {_at("entity_membership", snap, "m")}
        LEFT JOIN {_at("golden_records", snap, "g")} ON g.entity_id = m.entity_id
        WHERE true {after}
        GROUP BY m.entity_id
        HAVING count(*) >= 2
        ORDER BY m.entity_id
        LIMIT {int(limit) + 1}
    """
    rows = _rows(connection, sql, params)
    next_cursor = None
    if len(rows) > limit:
        rows = rows[:limit]
        next_cursor = encode_cursor(snap, str(rows[-1]["entity_id"]))
    return rows, snap, next_cursor


def runs_list(connection: duckdb.DuckDBPyConnection, *, limit: int = 20) -> list[dict[str, Any]]:
    return _rows(
        connection,
        f"SELECT run_id, mode, status, started_at, ended_at, config_hash, model_version, "
        f"tf_snapshot_id, rebuild_reason FROM {SCHEMA_QUALIFIER}.runs "
        f"ORDER BY run_id DESC LIMIT {int(limit)}",
        [],
    )


def metrics(connection: duckdb.DuckDBPyConnection) -> dict[str, Any]:
    """The dashboard numbers: corpus size, entities, duplicates, review backlog."""

    def scalar(sql: str) -> int:
        row = connection.execute(sql).fetchone()
        return 0 if row is None or row[0] is None else int(row[0])

    entities = scalar(f"SELECT count(*) FROM {SCHEMA_QUALIFIER}.golden_records")
    records = scalar(f"SELECT count(*) FROM {SCHEMA_QUALIFIER}.entity_membership")
    duplicates = scalar(
        f"SELECT count(*) FROM (SELECT entity_id FROM {SCHEMA_QUALIFIER}.entity_membership "
        f"GROUP BY entity_id HAVING count(*) >= 2)"
    )
    open_reviews = scalar(
        f"SELECT count(*) FROM {SCHEMA_QUALIFIER}.review_queue WHERE status = 'open'"
    )
    return {
        "records": records,
        "entities": entities,
        "duplicate_groups": duplicates,
        "records_in_duplicate_groups": records - (entities - duplicates),
        "open_reviews": open_reviews,
        "snapshot": current_snapshot(connection),
    }


def merge_plans(
    connection: duckdb.DuckDBPyConnection, *, since: str | None = None, limit: int = 500
) -> list[dict[str, Any]]:
    """Actionable merge plans from the event ledger (docs/backend-design.md §10).

    One plan per entity that gained members (``merged`` / ``member_added``
    events, optionally since a watermark): the elected master is the member
    record contributing the most golden attributes per ``golden_lineage``
    (tie: lowest record_key — deterministic), the victims are the rest, and
    the field updates are the golden values themselves. This is the insights-
    mode export a buyer can act on before writeback exists.
    """
    params: list[Any] = []
    since_clause = ""
    if since is not None:
        since_clause = "AND e.occurred_at >= ?"
        params.append(since)
    sql = f"""
        WITH affected AS (
          SELECT DISTINCT e.entity_id
          FROM {SCHEMA_QUALIFIER}.entity_events e
          WHERE e.event_type IN ('merged', 'member_added') {since_clause}
        ),
        contributions AS (
          SELECT l.entity_id, l.record_key, count(*) AS won_attributes
          FROM {SCHEMA_QUALIFIER}.golden_lineage l
          JOIN affected a ON a.entity_id = l.entity_id
          GROUP BY l.entity_id, l.record_key
        ),
        masters AS (
          SELECT entity_id, record_key AS master_key
          FROM (
            SELECT entity_id, record_key,
                   row_number() OVER (
                     PARTITION BY entity_id
                     ORDER BY won_attributes DESC, record_key
                   ) AS rank
            FROM contributions
          ) WHERE rank = 1
        )
        SELECT m.entity_id,
               ms.master_key,
               list(m.source_system || ':' || m.source_record_id ORDER BY m.record_key)
                 AS member_records,
               count(*) AS member_count,
               any_value(g.given_name) AS golden_given_name,
               any_value(g.family_name) AS golden_family_name,
               any_value(g.email) AS golden_email,
               any_value(g.phone_e164) AS golden_phone_e164
        FROM {SCHEMA_QUALIFIER}.entity_membership m
        JOIN masters ms ON ms.entity_id = m.entity_id
        LEFT JOIN {SCHEMA_QUALIFIER}.golden_records g ON g.entity_id = m.entity_id
        GROUP BY m.entity_id, ms.master_key
        HAVING count(*) >= 2
        ORDER BY m.entity_id
        LIMIT {int(limit)}
    """
    return _rows(connection, sql, params)
