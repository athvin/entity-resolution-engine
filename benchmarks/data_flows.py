"""Isolated SQL/reference comparisons; run each case in a fresh process.

Example: python benchmarks/data_flows.py --case ingest --mode sql --records 1000000
The reference mode uses the preserved pre-pushdown APIs and identical input data.
These are stage microbenchmarks, not full-pipeline throughput claims.
"""

from __future__ import annotations

import argparse
import json
import resource
import signal
import sys
import tempfile
import time
from datetime import datetime
from pathlib import Path
from typing import Any

import duckdb

from er.config.loader import load_config
from er.config.schema import Thresholds
from er.entities.ids import CountingIdFactory
from er.lake.model import REGISTRY, create_table_sql

ROOT = Path(__file__).resolve().parents[1]
STAMP = datetime(2026, 1, 1)


def run_case(case: str, mode: str, records: int, directory: Path) -> dict[str, Any]:
    cfg = load_config(ROOT / "configs/test.yaml")
    with duckdb.connect(config={"threads": 2, "memory_limit": "512MB"}) as connection:
        connection.execute("SET temp_directory = ?", [str(directory / "spill")])
        connection.execute("ATTACH ':memory:' AS lake")
        if case == "ingest":
            from er.ingest.landing import STAGING_TABLE, _stage_delivery
            from er.ingest.sources import CsvAdapter

            (directory / "crm").mkdir()
            connection.execute(
                "COPY (SELECT i::VARCHAR crm_id, 'Name' || i AS first_name, "
                "'Family' || i AS last_name, 'email' || i || '@example.org' AS email_address "
                "FROM range($n) t(i)) TO $path (FORMAT CSV, HEADER)",
                {"n": records, "path": str(directory / "crm/data.csv")},
            )
            adapter = CsvAdapter("crm", cfg.sources["crm"], directory)

            def operation() -> None:
                if mode == "sql":
                    from er.ingest.landing import _stage_files

                    _stage_files(connection, adapter)
                else:
                    _stage_delivery(connection, adapter.rows(), adapter.columns)

            fingerprint_query = (
                "SELECT count(*), bit_xor(hash(source_record_id, content_hash)) "
                f"FROM {STAGING_TABLE}"
            )
        elif case == "review":
            from er.matching.full import _scored_rows, review_scored_pairs

            connection.execute(create_table_sql(REGISTRY["review_queue"]))
            connection.execute(
                "CREATE TABLE lake.main.match_scores AS SELECT 'crm:' || i AS rec_a_key, "
                "'web:' || i AS rec_b_key, CASE WHEN i%5<2 THEN 0.7 ELSE 0.99 END::DOUBLE "
                "AS match_probability, json_object('gamma_email', 2, 'bf_email', 4.5) AS evidence, "
                "'v1' AS model_version, 'tf1' AS tf_snapshot_id, 'run' AS run_id "
                "FROM range(?) t(i)",
                [records],
            )

            def operation() -> None:
                thresholds = Thresholds(review_low=0.5, auto_merge=0.9)
                if mode == "sql":
                    from er.matching.full import review_score_relation

                    review_score_relation(
                        connection,
                        thresholds,
                        model_version="v1",
                        tf_snapshot_id="tf1",
                        run_id="run",
                        id_factory=CountingIdFactory(),
                    )
                else:
                    review_scored_pairs(
                        connection,
                        _scored_rows(
                            connection, model_version="v1", tf_snapshot_id="tf1", run_id="run"
                        ),
                        thresholds,
                        run_id="run",
                        id_factory=CountingIdFactory(),
                    )

            fingerprint_query = (
                "SELECT count(*), "
                "bit_xor(hash(review_id, rec_a_key, rec_b_key, match_probability)) "
                "FROM lake.main.review_queue"
            )
        elif case == "tf":
            from splink import DuckDBAPI, Linker

            from er.matching.tf import register_tf, tf_columns

            columns = tf_columns(cfg)
            connection.execute(create_table_sql(REGISTRY["tf_lookup"]))
            connection.execute(
                "INSERT INTO lake.main.tf_lookup SELECT 'v1', 'tf1', "
                "list_extract(?::VARCHAR[], (i % ?) + 1), i::VARCHAR, 0.00001 "
                "FROM range(?) t(i)",
                [list(columns), len(columns), records],
            )
            connection.execute("CREATE TABLE corpus AS SELECT 'crm:1' record_key")
            linker = Linker(
                "corpus",
                {
                    "link_type": "dedupe_only",
                    "unique_id_column_name": "record_key",
                    "comparisons": [],
                },
                db_api=DuckDBAPI(connection),
                set_up_basic_logging=False,
            )

            def operation() -> None:
                if mode == "sql":
                    register_tf(linker, connection, cfg, model_version="v1", tf_snapshot_id="tf1")
                else:
                    # Exact former TF boundary: fetched tuples -> dicts -> Splink registration.
                    for column in columns:
                        rows = connection.execute(
                            "SELECT value, tf_value FROM lake.main.tf_lookup "
                            "WHERE model_version='v1' AND tf_snapshot_id='tf1' AND column_name=?",
                            [column],
                        ).fetchall()
                        linker.table_management.register_term_frequency_lookup(
                            [{column: str(value), f"tf_{column}": float(tf)} for value, tf in rows],
                            col_name=column,
                            overwrite=True,
                        )

            fingerprint_query = ""

        else:
            from er.entities.reconcile import reconcile_plan
            from er.entities.reconcile_stage import apply_reconcile_plan

            for name in ("entities", "entity_membership", "entity_events"):
                connection.execute(create_table_sql(REGISTRY[name]))
            connection.execute(
                "CREATE TEMP TABLE prior AS SELECT 'crm:' || printf('%09d', i) "
                "record_key, 'old' || (i // 4) entity_id FROM range(?) t(i)",
                [records],
            )
            connection.execute(
                "CREATE TEMP TABLE old_entities AS SELECT DISTINCT entity_id FROM prior"
            )
            connection.execute(
                "CREATE TEMP TABLE labels AS SELECT record_key, "
                "min(record_key) OVER (PARTITION BY i // 5) AS label "
                "FROM (SELECT *, row_number() OVER (ORDER BY record_key)-1 i FROM prior)"
            )

            def operation() -> None:
                ids = CountingIdFactory()
                if mode == "sql":
                    from er.entities.events import append_events
                    from er.entities.reconcile_stage import (
                        _merge_entity_relation,
                        _merge_membership_relation,
                    )
                    from er.entities.relational import event_stream, lifecycle_plan

                    with lifecycle_plan(connection, "prior", "old_entities", "labels", ids) as plan:
                        _merge_entity_relation(connection, plan.entities, STAMP, "run")
                        _merge_membership_relation(connection, plan.membership, STAMP, "run")
                        append_events(
                            connection,
                            event_stream(
                                connection, plan.events, run_id="run", ids=ids, reason=None
                            ),
                            occurred_at=STAMP,
                        )
                else:
                    old: dict[str, set[str]] = {}
                    groups: dict[str, set[str]] = {}
                    for key, entity in connection.execute("SELECT * FROM prior").fetchall():
                        old.setdefault(entity, set()).add(key)
                    for key, label in connection.execute("SELECT * FROM labels").fetchall():
                        groups.setdefault(label, set()).add(key)
                    plan = reconcile_plan(old, groups.values(), ids)
                    apply_reconcile_plan(connection, plan, run_id="run", occurred_at=STAMP, ids=ids)

            fingerprint_query = (
                "SELECT count(*), "
                "bit_xor(hash(event_id, entity_id, event_type, seq, details_hash)) "
                "FROM lake.main.entity_events"
            )
        started = time.perf_counter()
        operation()
        elapsed = time.perf_counter() - started
        # Capture before validation, which is deliberately outside the timed region.
        rss = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
        rss_bytes = int(rss if sys.platform == "darwin" else rss * 1024)
        if case == "tf":
            sources = []
            for column in columns:
                name = linker._intermediate_table_cache[f"__splink__df_tf_{column}"].physical_name
                sources.append(
                    f"SELECT '{column}' column_name, {column} AS value, "
                    f"tf_{column} AS tf_value FROM {name}"
                )
            fingerprint_query = "SELECT count(*), bit_xor(hash(column_name,value,tf_value)) FROM ("
            fingerprint_query += " UNION ALL ".join(sources) + ")"
        fingerprint = connection.execute(fingerprint_query).fetchone()
        return {
            "case": case,
            "mode": mode,
            "records": records,
            "seconds": elapsed,
            "peak_rss_bytes": rss_bytes,
            "fingerprint": fingerprint,
            "duckdb": duckdb.__version__,
            "threads": 2,
            "memory_limit": "512MB",
        }


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--case", choices=("ingest", "review", "tf", "reconcile"), required=True)
    parser.add_argument("--mode", choices=("sql", "reference"), required=True)
    parser.add_argument("--records", type=int, default=100_000)
    parser.add_argument("--timeout", type=int, default=120, help="Stop after this many seconds")
    args = parser.parse_args()

    def timed_out(_signal: int, _frame: object) -> None:
        raise TimeoutError(f"{args.case}/{args.mode} exceeded {args.timeout} seconds")

    signal.signal(signal.SIGALRM, timed_out)
    signal.alarm(args.timeout)
    with tempfile.TemporaryDirectory(prefix="er-data-flows-") as scratch:
        print(json.dumps(run_case(args.case, args.mode, args.records, Path(scratch))))
