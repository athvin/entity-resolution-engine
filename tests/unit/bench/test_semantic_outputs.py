"""Streaming benchmark artifacts preserve the original canonical byte format."""

import gzip
import hashlib
import importlib
import json
import sys
from pathlib import Path
from types import SimpleNamespace

import duckdb
import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[3] / "benchmarks"))
semantic = importlib.import_module("semantic_outputs")


@pytest.mark.parametrize("size", [0, 2051])
def test_streaming_artifacts_match_collection_format(tmp_path: Path, size: int) -> None:
    with duckdb.connect() as connection:
        connection.execute("ATTACH ':memory:' AS lake")
        connection.execute(
            "CREATE TABLE lake.main.entity_membership AS SELECT 'e' entity_id, "
            "'crm:' || i AS record_key FROM range(?) t(i)",
            [size],
        )
        connection.execute(
            "CREATE TABLE lake.main.int_std_records AS SELECT record_key, "
            "'é 雪' AS name, 1e-7::DOUBLE AS value, NULL::VARCHAR absent, "
            "TIMESTAMP '2026-01-01 03:04:05.006' AS at, 'batch' AS ingest_batch_id, "
            "TIMESTAMP '2026-01-02' AS ingested_at FROM lake.main.entity_membership"
        )
        connection.execute(
            "CREATE TABLE lake.main.int_blocking_keys AS SELECT record_key, "
            "'email' AS key_type, record_key AS key_value "
            "FROM lake.main.entity_membership"
        )
        for table in ("golden_records", "golden_lineage"):
            connection.execute(
                f"CREATE TABLE lake.main.{table} AS SELECT DISTINCT entity_id, "
                "'O''Brien' AS value, TIMESTAMP '2026-01-03' assembled_at "
                "FROM lake.main.entity_membership"
            )
        connection.execute(
            "CREATE TABLE lake.main.match_scores AS SELECT record_key rec_a_key, "
            "'web:' || record_key AS rec_b_key, 0.7::DOUBLE match_probability, "
            "'a' rec_a_content_hash, 'b' rec_b_content_hash, true is_active "
            "FROM lake.main.entity_membership"
        )
        config = SimpleNamespace(thresholds=SimpleNamespace(auto_merge=0.9, review_low=0.5))
        actual = semantic.save_semantic_outputs(connection, config, tmp_path, "base")
        labels = dict(
            connection.execute(
                "SELECT entity_id, min(record_key) "
                "FROM lake.main.entity_membership GROUP BY entity_id"
            ).fetchall()
        )
        for table, ignored in {
            "int_std_records": {"ingest_batch_id", "ingested_at"},
            "int_blocking_keys": set(),
            "golden_records": {"assembled_at"},
            "golden_lineage": {"assembled_at"},
        }.items():
            cursor = connection.execute(f"SELECT * FROM lake.main.{table}")
            names = [column[0] for column in cursor.description]
            rows = [
                json.dumps(
                    {
                        name: labels[value] if name == "entity_id" else value
                        for name, value in zip(names, row, strict=True)
                        if name not in ignored
                    },
                    sort_keys=True,
                    default=str,
                    ensure_ascii=False,
                )
                for row in cursor.fetchall()
            ]
            assert actual[table] == hashlib.sha256("\n".join(sorted(rows)).encode()).hexdigest()
        assert actual["candidate_pairs"] == hashlib.sha256(b"[]").hexdigest()
        scores = connection.execute("SELECT * FROM lake.main.match_scores ORDER BY 1,2").fetchall()
        with gzip.open(tmp_path / "base-scores.json.gz", "rt") as handle:
            assert handle.read() == json.dumps(scores)
        classes = [(r[0], r[1], r[2] >= 0.9, r[2] >= 0.5, *r[3:]) for r in scores]
        assert (
            actual["score_classifications"]
            == hashlib.sha256(json.dumps(classes).encode()).hexdigest()
        )
