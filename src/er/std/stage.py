"""The real standardization stage, shared by the CLI and profiling campaign."""

from __future__ import annotations

from typing import Any

import duckdb

from er.config.schema import Config
from er.dbt_runner import DbtResult, render_dbt_vars, run_dbt
from er.errors import ExitCode
from er.lake.ddl import create_statement
from er.lake.ducklake import connect
from er.obs.profiling import span
from er.obs.runctx import StageRun
from er.std.blocking import BLOCKING_DBT_VAR, blocking_payload

WORK_RELATION = "lake.main.er_standardize_work"


def _exists(connection: duckdb.DuckDBPyConnection, relation: str) -> bool:
    row = connection.execute(
        "SELECT EXISTS (SELECT 1 FROM duckdb_tables() "
        "WHERE database_name='lake' AND schema_name='main' AND table_name=?)",
        [relation],
    ).fetchone()
    assert row is not None
    return bool(row[0])


def prepare_work(
    connection: duckdb.DuckDBPyConnection,
    cfg: Config,
    run_id: str,
    *,
    full_refresh: bool,
    changed_only: bool = True,
) -> tuple[int, bool, bool]:
    """Journal batches before staging; unfinished work survives retries and new runs.

    The tenant writer lock covers preparation, dbt and acknowledgement. Historical
    journal rows are copied to the current run, never acknowledged by staging alone.
    """
    connection.execute(create_statement("er_standardize_work"))
    refresh_row = connection.execute(
        f"SELECT coalesce(bool_or(full_refresh), false) FROM {WORK_RELATION}"
    ).fetchone()
    assert refresh_row is not None
    full_refresh = full_refresh or bool(refresh_row[0])
    delta = (
        changed_only
        and not full_refresh
        and all(_exists(connection, name) for name in ("int_std_records", "int_blocking_keys"))
    )
    connection.execute("BEGIN TRANSACTION")
    try:
        connection.execute(
            f"INSERT INTO {WORK_RELATION} SELECT DISTINCT ?, w.source_system, "
            f"w.ingest_batch_id, ? FROM {WORK_RELATION} w WHERE NOT EXISTS "
            f"(SELECT 1 FROM {WORK_RELATION} current WHERE current.run_id=? "
            "AND current.source_system=w.source_system "
            "AND current.ingest_batch_id=w.ingest_batch_id)",
            [run_id, full_refresh, run_id],
        )
        for source in cfg.sources:
            relation = f"stg_{source}"
            predicate = (
                f"AND r.ingest_batch_id NOT IN (SELECT DISTINCT ingest_batch_id "
                f'FROM lake.main."{relation}")'
                if delta and _exists(connection, relation)
                else ""
            )
            connection.execute(
                f"INSERT INTO {WORK_RELATION} SELECT DISTINCT ?, r.source_system, "
                "r.ingest_batch_id, ? FROM lake.main.raw_records r "
                f"WHERE r.source_system=? {predicate} AND NOT EXISTS "
                f"(SELECT 1 FROM {WORK_RELATION} w WHERE w.run_id=? "
                "AND w.source_system=r.source_system AND w.ingest_batch_id=r.ingest_batch_id)",
                [run_id, full_refresh, source, run_id],
            )
        if full_refresh:
            connection.execute(
                f"UPDATE {WORK_RELATION} SET full_refresh=true WHERE run_id=?", [run_id]
            )
        connection.execute("COMMIT")
    except BaseException:
        connection.execute("ROLLBACK")
        raise
    pending_row = connection.execute(
        f"SELECT count(*) FROM lake.main.raw_records r JOIN {WORK_RELATION} w "
        "ON w.source_system=r.source_system AND w.ingest_batch_id=r.ingest_batch_id "
        "WHERE w.run_id=?",
        [run_id],
    ).fetchone()
    assert pending_row is not None
    pending = int(pending_row[0])
    return pending, delta, full_refresh


def _count(connection: duckdb.DuckDBPyConnection, relation: str) -> int:
    exists = connection.execute(
        "SELECT count(*) FROM duckdb_tables() WHERE database_name='lake' AND table_name=?",
        [relation],
    ).fetchone()
    if not exists or not exists[0]:
        return 0
    row = connection.execute(f'SELECT count(*) FROM lake.main."{relation}"').fetchone()
    return int(row[0]) if row else 0


def standardize(
    cfg: Config,
    stage: StageRun,
    *,
    changed_only: bool,
    full_refresh: bool = False,
) -> tuple[int, DbtResult | None]:
    with span("standardize.input_counts", unit="records") as metrics, connect() as connection:
        before = {f"stg_{source}": _count(connection, f"stg_{source}") for source in cfg.sources}
        pending, delta, full_refresh = prepare_work(
            connection, cfg, stage.run_id, full_refresh=full_refresh, changed_only=changed_only
        )
        raw = _count(connection, "raw_records")
        seeded = _count(connection, "nickname_variants") > 0
        metrics.update(rows_pending=pending, raw_rows_total=raw)
        if changed_only and delta and pending == 0:
            stage.counters.set("rows_in", 0)
            stage.counters.set("rows_out", _count(connection, "int_std_records"))
            stage.counters.set("models_run", 0)
            return int(ExitCode.NOTHING_TO_DO), None

    variables = render_dbt_vars(
        cfg, stage.run_id, {BLOCKING_DBT_VAR: blocking_payload(cfg), "standardize_delta": delta}
    )
    selector = "staging intermediate" + (" nickname_variants" if not seeded else "")
    with span("standardize.dbt_build", unit="records"):
        result = run_dbt("build", select=selector, vars=variables, full_refresh=full_refresh)
    with span("standardize.output_counts", unit="records") as metrics, connect() as connection:
        std = _count(connection, "int_std_records")
        blocking = _count(connection, "int_blocking_keys")
        after = {name: _count(connection, name) for name in before}
        keys = dict(
            connection.execute(
                "SELECT key_type, count(*) FROM lake.main.int_blocking_keys GROUP BY key_type"
            ).fetchall()
        )
        deleted = connection.execute(
            "SELECT count(*) FROM (SELECT is_deleted, row_number() OVER (PARTITION BY "
            "source_system, source_record_id ORDER BY ingested_at DESC, ingest_batch_id DESC) rn "
            "FROM lake.main.raw_records) WHERE rn=1 AND is_deleted"
        ).fetchone()
        counters: dict[str, Any] = {
            "rows_in": raw if full_refresh else pending,
            "rows_out": std,
            "models_run": sum(model.unique_id.startswith("model.") for model in result.models),
            "stg_rows_appended": sum(after.values())
            if full_refresh
            else sum(after[name] - before[name] for name in before),
            "std_rows_current": std,
            "blocking_rows": blocking,
            "blocking_keys_by_type": keys,
            "tombstones_excluded": int(deleted[0]) if deleted else 0,
        }
        for name, value in counters.items():
            stage.counters.set(name, value)
        metrics.update(counters)
        # No other writer can add work while this stage owns the tenant lock.
        # Leave the journal intact on any dbt/counting failure.
        connection.execute(f"DELETE FROM {WORK_RELATION}")
    return int(ExitCode.SUCCESS), result
