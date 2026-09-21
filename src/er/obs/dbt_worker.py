"""Isolated dbt entry point: profile native cursors without altering installed packages."""

from __future__ import annotations

import json
import sys
import time
from typing import Any

from er.obs.profiling import emit, span
from er.obs.sql_profile import instrument_connection


def main() -> None:
    from dbt.adapters.duckdb.connections import DuckDBConnectionManager
    from dbt.adapters.duckdb.environments.local import LocalEnvironment
    from dbt.cli.main import dbtRunner

    environment_class: Any = LocalEnvironment
    original: Any = environment_class.handle
    handles: list[Any] = []
    variables = json.loads(sys.argv[sys.argv.index("--vars") + 1]) if "--vars" in sys.argv else {}
    before: dict[str, dict[str, Any]] = {}
    spans: dict[str, tuple[Any, dict[str, Any]]] = {}

    def model_counts(name: str) -> dict[str, Any]:
        started = time.monotonic()
        environment: Any = DuckDBConnectionManager.env()
        cursor = environment.conn.cursor()
        try:
            cursor.disable_profiling()

            def count(table: str, predicate: str = "TRUE") -> int:
                exists = cursor.execute(
                    "SELECT count(*) FROM information_schema.tables "
                    "WHERE table_catalog='lake' AND table_name=?",
                    [table],
                ).fetchone()
                if not exists or not exists[0]:
                    return 0
                return int(
                    cursor.execute(
                        f'SELECT count(*) FROM lake.main."{table}" WHERE {predicate}'
                    ).fetchone()[0]
                )

            total = count(name)
            if name.startswith("stg_"):
                source = name.removeprefix("stg_")
                predicate = f"source_system='{source}'"
                if total:
                    predicate += (
                        " AND ingest_batch_id NOT IN (SELECT DISTINCT ingest_batch_id "
                        f'FROM lake.main."{name}")'
                    )
                inputs = count("raw_records", predicate)
            elif name == "int_std_records":
                inputs = sum(count(f"stg_{source}") for source in variables.get("sources", {}))
            elif name == "int_blocking_keys":
                inputs = count("int_std_records")
            elif name in ("golden_records", "golden_lineage"):
                predicate = "TRUE"
                if variables.get("assemble_touched_only"):
                    run_id = variables["run_id"]
                    predicate = (
                        "entity_id IN (SELECT entity_id FROM lake.main.er_touched_entities "
                        f"WHERE run_id='{run_id}' AND disposition='rebuild')"
                    )
                inputs = count("entity_membership", predicate)
                total = count(name, predicate)
            else:
                inputs = count("golden_records")
            return {"rows_in": inputs, "rows_out": total}
        finally:
            cursor.close()
            emit(
                "count_probe",
                name=f"dbt.{name}.counts",
                duration_ms=(time.monotonic() - started) * 1000,
            )

    def callback(event: Any) -> None:
        if event.info.name not in ("NodeStart", "NodeFinished"):
            return
        node = event.data.node_info
        if node.resource_type != "model":
            return
        name = node.node_name
        try:
            if event.info.name == "NodeStart":
                manager = span(f"dbt.{name}", dbt_model=node.unique_id)
                spans[name] = manager, manager.__enter__()
            counts = model_counts(name)
            counts["input_unit"] = "entities" if name == "golden_display" else "records"
            counts["output_unit"] = (
                "keys"
                if name == "int_blocking_keys"
                else "attributes"
                if name == "golden_lineage"
                else "entities"
                if name.startswith("golden_")
                else "records"
            )
            if event.info.name == "NodeStart":
                before[name] = counts
                emit("dbt_model_start", name=node.unique_id, metrics=counts)
            else:
                initial = before.get(name, {})
                counts["rows_in"] = initial.get("rows_in")
                counts["rows_written"] = (
                    counts["rows_out"] - initial.get("rows_out", 0)
                    if name.startswith("stg_")
                    else counts["rows_out"]
                )
                emit("dbt_model_end", name=node.unique_id, metrics=counts)
                manager, metrics = spans.pop(name)
                metrics.update(counts)
                manager.__exit__(None, None, None)
        except Exception as error:
            emit("dbt_counts_error", name=name, error_detail=str(error))

    def handle(environment: Any) -> Any:
        value = original(environment)
        value._cursor._cursor = instrument_connection(value._cursor._cursor)
        handles.append(value)
        return value

    environment_class.handle = handle
    try:
        result = dbtRunner(callbacks=[callback]).invoke(sys.argv[1:])
        if result.exception is not None:
            print(str(result.exception), file=sys.stderr)
        raise SystemExit(0 if result.success else 1)
    finally:
        for value in handles:
            value._cursor._cursor.close()
        environment_class.handle = original


if __name__ == "__main__":
    main()
