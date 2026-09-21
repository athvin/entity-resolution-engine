"""The real standardization stage, shared by the CLI and profiling campaign."""

from __future__ import annotations

from typing import Any

import duckdb

from er.config.schema import Config
from er.dbt_runner import DbtResult, render_dbt_vars, run_dbt
from er.errors import ExitCode
from er.lake.ducklake import connect
from er.obs.profiling import span
from er.obs.runctx import StageRun
from er.std.blocking import BLOCKING_DBT_VAR, blocking_payload


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
        pending = 0
        for source in cfg.sources:
            relation = f"stg_{source}"
            # Empty and absent staging relations both have no processed rows.
            predicate = (
                f"AND ingest_batch_id NOT IN (SELECT DISTINCT ingest_batch_id "
                f'FROM lake.main."{relation}")'
                if before[relation]
                else ""
            )
            row = connection.execute(
                f"SELECT count(*) FROM lake.main.raw_records WHERE source_system=? {predicate}",
                [source],
            ).fetchone()
            pending += int(row[0]) if row else 0
        raw = _count(connection, "raw_records")
        seeded = _count(connection, "nickname_variants") > 0
        metrics.update(rows_pending=pending, raw_rows_total=raw)
        if changed_only and not full_refresh and pending == 0:
            stage.counters.set("rows_in", 0)
            stage.counters.set("rows_out", _count(connection, "int_std_records"))
            stage.counters.set("models_run", 0)
            return int(ExitCode.NOTHING_TO_DO), None

    variables = render_dbt_vars(cfg, stage.run_id, {BLOCKING_DBT_VAR: blocking_payload(cfg)})
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
    return int(ExitCode.SUCCESS), result
