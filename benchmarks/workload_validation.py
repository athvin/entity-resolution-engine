"""Deferred, snapshot-based validation and storage evidence for paired workloads."""

from __future__ import annotations

import hashlib
import json
import os
import re
from pathlib import Path
from typing import Any

import duckdb
from psycopg import sql

from er.lake.catalog import catalog_connect
from er.lake.ducklake import current_snapshot
from er.lake.objectstore import ObjectStore


def compare_score_files(left: Path, right: Path) -> dict[str, Any]:
    """Bounded offline comparison; DuckDB reads gzip arrays and owns the join."""
    with duckdb.connect() as connection:
        connection.execute("SET threads=2")
        connection.execute("SET memory_limit='1GB'")
        for table, path in (("a", left), ("b", right)):
            connection.execute(
                f"CREATE TABLE {table} AS SELECT json->>0 AS rec_a, json->>1 AS rec_b, "
                "(json->>2)::DOUBLE AS probability, json->>3 AS hash_a, json->>4 AS hash_b, "
                "(json->>5)::BOOLEAN AS active "
                "FROM read_json_objects(?, format='array')",
                [str(path)],
            )
        count, mismatches, maximum = connection.execute(
            "SELECT count(*), count(*) FILTER (WHERE a.rec_a IS NULL OR b.rec_a IS NULL "
            "OR a.hash_a IS DISTINCT FROM b.hash_a OR a.hash_b IS DISTINCT FROM b.hash_b "
            "OR a.active IS DISTINCT FROM b.active OR NOT isfinite(a.probability) "
            "OR NOT isfinite(b.probability) OR a.probability IS NULL OR b.probability IS NULL "
            "OR abs(a.probability-b.probability)>1e-10), "
            "coalesce(max(abs(a.probability-b.probability)),0) "
            "FROM a FULL JOIN b USING(rec_a, rec_b)"
        ).fetchone()
    return {
        "pairs": count,
        "mismatches": mismatches,
        "max_probability_delta": maximum,
        "tolerance": 1e-10,
        "status": "passed" if not mismatches else "failed",
    }


class SnapshotReader:
    """Apply time travel to the trusted benchmark helpers' FROM/JOIN lake references.

    Temporary SQL relations remain writable for bounded validation. This is not a
    general SQL rewriter or a production connection: persistent writes are refused.
    """

    def __init__(self, connection: Any, snapshot: int):
        self.connection = connection
        self.snapshot = int(snapshot)

    def __getattr__(self, name: str) -> Any:
        return getattr(self.connection, name)

    def execute(self, query: str, parameters: Any = None, **kwargs: Any) -> Any:
        if re.search(r"\b(?:INTO|UPDATE|TABLE|DELETE\s+FROM)\s+lake\.", query, re.I):
            raise ValueError("snapshot validation refuses persistent lake writes")
        query = re.sub(
            r'\b(FROM|JOIN)\s+(lake\.main\.(?:"\w+"|\w+))(?![\w.(])',
            lambda match: f"{match[1]} (SELECT * FROM {match[2]} AT (VERSION => {self.snapshot}))",
            query,
            flags=re.I,
        )
        return self.connection.execute(query, parameters, **kwargs)


def checkpoint(connection: Any) -> dict[str, Any]:
    row = connection.execute(
        "SELECT model_version, tf_snapshot_id FROM lake.main.model_registry WHERE status='active'"
    ).fetchone()
    if row is None:
        raise AssertionError("workload has no active model")
    return {
        "snapshot": current_snapshot(connection),
        "model_version": row[0],
        "tf_snapshot_id": row[1],
    }


def model_fingerprint(connection: Any, directory: Path, phase: str) -> dict[str, str]:
    row = connection.execute(
        "SELECT params_path FROM lake.main.model_registry WHERE status='active'"
    ).fetchone()
    model = ObjectStore.from_env().get_bytes(row[0])
    (directory / f"{phase}-model.json").write_bytes(model)
    path = directory / f"{phase}-frozen-tf.csv"
    connection.execute(
        "COPY (SELECT column_name, value, tf_value FROM lake.main.tf_lookup "
        "ORDER BY column_name, value) TO ? (FORMAT CSV, HEADER true)",
        [str(path)],
    )
    with path.open("rb") as handle:
        tf_hash = hashlib.file_digest(handle, "sha256").hexdigest()
    return {"model_sha256": hashlib.sha256(model).hexdigest(), "tf_sha256": tf_hash}


def capture_layout(connection: Any, directory: Path, phase: str, snapshot: int) -> dict[str, Any]:
    """Inventory only files visible at the checkpoint; read footers outside timing."""
    document: dict[str, Any] = {"snapshot": snapshot, "tables": {}, "capabilities": {}}

    def rows(query: str, params: Any = None) -> list[dict[str, Any]]:
        cursor = connection.execute(query, params)
        names = [column[0] for column in cursor.description]
        return [dict(zip(names, row, strict=True)) for row in cursor.fetchall()]

    for name, query in {
        "options": "SELECT * FROM ducklake_options('lake')",
        "current_table_info": "SELECT * FROM ducklake_table_info('lake')",
        "settings": "SELECT name, value FROM duckdb_settings() WHERE name IN "
        "('threads','memory_limit','temp_directory','preserve_insertion_order') "
        "OR name LIKE 'ducklake%'",
    }.items():
        try:
            document[name] = rows(query)
            document["capabilities"][name] = "available"
        except Exception as error:
            document["capabilities"][name] = f"unavailable: {error}"
    # Read-only catalog evidence, including version columns when supported.
    # These are all metadata rows, not a scan of tenant record contents.
    document["catalog_layout"] = {}
    with catalog_connect() as catalog:
        for table in (
            "ducklake_table",
            "ducklake_column",
            "ducklake_partition_info",
            "ducklake_partition_column",
            "ducklake_sort_info",
            "ducklake_sort_expression",
        ):
            try:
                cursor = catalog.execute(
                    sql.SQL("SELECT * FROM {}.{}").format(
                        sql.Identifier(os.environ["ER_LAKE_METADATA_SCHEMA"]),
                        sql.Identifier(table),
                    )
                )
                names = [column.name for column in cursor.description]
                document["catalog_layout"][table] = [
                    dict(zip(names, row, strict=True)) for row in cursor.fetchall()
                ]
                document["capabilities"][table] = "available"
            except Exception as error:
                document["capabilities"][table] = f"unavailable: {error}"
    # Table information is current metadata; the file list and footer evidence
    # are explicitly snapshot-specific. Do not label current options historical.
    for table in (
        "raw_records",
        "int_std_records",
        "int_blocking_keys",
        "match_scores",
        "entity_membership",
        "golden_records",
        "golden_lineage",
        "tf_lookup",
    ):
        files = rows(
            "SELECT data_file, data_file_size_bytes, delete_file, delete_file_size_bytes "
            "FROM ducklake_list_files('lake', ?, snapshot_version => ?)",
            [table, snapshot],
        )
        unique_data = {row["data_file"]: row["data_file_size_bytes"] for row in files}
        unique_deletes = {
            row["delete_file"]: row["delete_file_size_bytes"] for row in files if row["delete_file"]
        }
        entry = {
            "files": files,
            "data_files": len(unique_data),
            "delete_files": len(unique_deletes),
            "data_bytes": sum(unique_data.values()),
            "delete_bytes": sum(unique_deletes.values()),
            "file_sizes_bytes": sorted(unique_data.values()),
        }
        document["tables"][table] = entry
        if unique_data:
            path = directory / f"{phase}-{table}-parquet-metadata.parquet"
            connection.execute(
                "COPY (SELECT file_name, row_group_id, row_group_num_rows, path_in_schema, "
                "stats_min_value, stats_max_value, compression FROM parquet_metadata($files)) "
                "TO $destination (FORMAT PARQUET)",
                {"files": list(unique_data), "destination": str(path)},
            )
            entry["parquet_metadata"] = path.name
    (directory / f"{phase}-lake-layout.json").write_text(
        json.dumps(document, indent=2, default=str)
    )
    return document


def query_coverage(directory: Path, commands: list[dict[str, Any]]) -> dict[str, Any]:
    """Verify consumed native SQL and substage coverage, with visible exclusions."""
    invocations = {entry["invocation_id"]: entry["phase"] for entry in commands}
    events = []
    for path in directory.glob("events-*.jsonl"):
        events.extend(json.loads(line) for line in path.read_text().splitlines() if line)
    submitted = {e["query_id"]: e for e in events if e["event"] == "sql_submitted"}
    consumed = {e["query_id"] for e in events if e["event"] == "sql_consumed"}
    rejected = {e["query_id"] for e in events if e["event"] == "sql_rejected"}
    profiles = [e for e in events if e["event"] == "sql_profile"]
    captured = {e["query_id"] for e in profiles}
    missing = consumed - captured - rejected
    # Native engine metadata/settings statements need not have an operator plan.
    excluded_kinds = {"SET", "PRAGMA", "TRANSACTION", "DROP", "ALTER", "CREATE", "EXPLAIN"}
    missing = {key for key in missing if submitted[key]["statement_kind"] not in excluded_kinds}
    required = {
        e["query_id"] for e in events if e["event"] == "sql_executed" and e.get("requires_profile")
    }
    missing |= required - captured - rejected
    exclusions = [
        {
            "query_id": key,
            "kind": event["statement_kind"],
            "reason": "rejected" if key in rejected else "metadata_or_unconsumed_result",
        }
        for key, event in submitted.items()
        if key not in captured and key not in missing
    ]
    errors = [e for e in events if e["event"].endswith("_error")]
    unidentified = [
        e for e in events if e["event"] == "sql_unattributed" and e.get("exclusion_reason") is None
    ]
    workloads = {}
    for phase in ("base", "batch"):
        selected = [e for e in profiles if invocations.get(e.get("invocation_id")) == phase]
        spans = [
            e
            for e in events
            if e["event"] == "span_end" and invocations.get(e.get("invocation_id")) == phase
        ]
        names = {e.get("name") for e in spans}
        required = {"ingest", "standardize", "match", "reconcile", "assemble"}
        if phase == "base":
            required |= {"train", "train.estimation_call", "match.full"}
        else:
            required |= {"match.incremental", "match.new_vs_corpus", "match.new_vs_new"}
        stages = required & {"ingest", "standardize", "train", "match", "reconcile", "assemble"}
        absent = required - names
        absent_sql = stages - {e.get("stage") for e in selected}
        expected_models = {
            f"model.er.{name}"
            for name in (
                "stg_crm",
                "stg_billing",
                "stg_webforms",
                "int_std_records",
                "int_blocking_keys",
                "golden_records",
                "golden_lineage",
                "golden_display",
            )
        }
        absent_models = expected_models - {e.get("dbt_model") for e in selected}
        # Every EM session and both incremental passes must have descendant SQL.
        parents = {e["span_id"]: e.get("parent_span_id") for e in spans}
        ancestors = set()
        for event in selected:
            cursor = event.get("span_id")
            while cursor and cursor not in ancestors:
                ancestors.add(cursor)
                cursor = parents.get(cursor)
        empty_estimators = [
            e["span_id"]
            for e in spans
            if (
                e.get("name") in ("match.new_vs_corpus", "match.new_vs_new")
                or (
                    e.get("name") == "train.estimation_call"
                    and str(e.get("method", "")).startswith("training.estimate")
                )
            )
            and e["span_id"] not in ancestors
        ]
        workloads[phase] = {
            "profiles": len(selected),
            "missing_spans": sorted(absent),
            "missing_sql_stages": sorted(absent_sql),
            "missing_sql_models": sorted(absent_models),
            "empty_estimators": empty_estimators,
        }
        if absent or absent_sql or absent_models or empty_estimators:
            errors.append({"phase": phase, **workloads[phase]})
    report = {
        "submitted": len(submitted),
        "profiles": len(profiles),
        "missing": sorted(missing),
        "exclusions": exclusions,
        "native_exclusions": [
            {key: e.get(key) for key in ("connection_id", "profile_path", "exclusion_reason")}
            for e in events
            if e["event"] == "sql_unattributed" and e.get("exclusion_reason")
        ],
        "unattributed": unidentified,
        "errors": errors,
        "workloads": workloads,
    }
    (directory / "query-coverage.json").write_text(json.dumps(report, indent=2))
    if missing or errors or unidentified:
        raise AssertionError("incomplete workload SQL coverage; see query-coverage.json")
    return report
