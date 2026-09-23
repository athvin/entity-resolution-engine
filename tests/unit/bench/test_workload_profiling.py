"""Trust boundaries for paired profiling: missing evidence must not pass."""

from __future__ import annotations

import gzip
import importlib
import json
import sys
from pathlib import Path
from unittest.mock import Mock

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[3] / "benchmarks"))
reporting = importlib.import_module("workload_report")
validation = importlib.import_module("workload_validation")


def test_snapshot_reader_pins_every_lake_read_and_refuses_writes() -> None:
    connection = Mock()
    reader = validation.SnapshotReader(connection, 19)
    reader.execute(
        "CREATE TEMP TABLE check_rows AS SELECT a.* FROM lake.main.int_std_records a "
        'JOIN lake.main."entity_membership" b USING(record_key) WHERE a.record_key=?',
        ["example"],
    )
    sql, parameters = connection.execute.call_args.args
    assert "int_std_records AT (VERSION => 19)) a" in sql
    assert '"entity_membership" AT (VERSION => 19)) b' in sql
    assert parameters == ["example"]
    with pytest.raises(ValueError, match="refuses persistent"):
        reader.execute("DELETE FROM lake.main.entity_membership")


def test_missing_consumed_query_cannot_pass_coverage(tmp_path: Path) -> None:
    events = [
        {"event": "sql_submitted", "query_id": "lost", "statement_kind": "SELECT"},
        {"event": "sql_consumed", "query_id": "lost"},
        {"event": "sql_submitted", "query_id": "write", "statement_kind": "CREATE"},
        {"event": "sql_executed", "query_id": "write", "requires_profile": True},
    ]
    (tmp_path / "events-1.jsonl").write_text("\n".join(map(json.dumps, events)))
    with pytest.raises(AssertionError, match="incomplete workload SQL coverage"):
        validation.query_coverage(tmp_path, [])
    report = json.loads((tmp_path / "query-coverage.json").read_text())
    assert report["missing"] == ["lost", "write"]
    assert report["workloads"]["batch"]["missing_sql_models"]


def test_missing_environment_values_are_not_equal_evidence() -> None:
    errors = reporting.comparison_errors({}, {})
    assert "NON_COMPARABLE: image_digest" in errors
    assert any("base_semantic_hashes" in error for error in errors)
    assert any("incremental/full equivalence" in error for error in errors)


def test_tuning_comparison_allows_only_the_explicit_image_change() -> None:
    before = {"fingerprint": {"image_digest": "sha256:before", "duckdb_version": "1.5.5"}}
    after = {"fingerprint": {"image_digest": "sha256:after", "duckdb_version": "1.5.4"}}
    assert "NON_COMPARABLE: image_digest" in reporting.comparison_errors(before, after)
    errors = reporting.comparison_errors(before, after, require_same_image=False)
    assert "NON_COMPARABLE: image_digest" not in errors
    assert "NON_COMPARABLE: duckdb_version" in errors
    assert any("incremental/full equivalence" in error for error in errors)


def test_only_learned_parameter_roundoff_is_permitted_between_fits() -> None:
    old = {"m_probability": 0.7, "threshold": 0.8}
    new = {**old, "m_probability": 0.7000000000000001}
    assert reporting.compare_model_parameters(old, new)["status"] == "passed"
    new["m_probability"] = 0.70000001
    assert reporting.compare_model_parameters(old, new)["status"] == "failed"
    assert (
        reporting.compare_model_parameters(old, {**old, "threshold": 0.8000000000000001})["status"]
        == "failed"
    )


def test_score_comparison_detects_missing_and_changed_pairs(tmp_path: Path) -> None:
    left, right = tmp_path / "a.json.gz", tmp_path / "b.json.gz"
    rows = [["one", "two", 0.8, "a", "b", True], ["two", "three", 0.6, "b", "c", True]]
    with gzip.open(left, "wt") as handle:
        json.dump(rows, handle)
    with gzip.open(right, "wt") as handle:
        json.dump([rows[0]], handle)
    assert validation.compare_score_files(left, right)["mismatches"] == 1
    rows[0][2] = 0.9
    with gzip.open(right, "wt") as handle:
        json.dump(rows, handle)
    assert validation.compare_score_files(left, right)["mismatches"] == 1


def test_offline_index_keeps_workloads_and_validation_separate(tmp_path: Path) -> None:
    sql = tmp_path / "sql"
    sql.mkdir()
    for phase, latency in (("base", 2), ("batch", 1), ("validation", 999)):
        document = {
            "query_id": phase,
            "invocation_id": phase,
            "phase": phase,
            "stage": "match",
            "profile": {
                "query_name": "SELECT 1",
                "latency": latency,
                "cumulative_rows_scanned": 10,
                "system_peak_temp_dir_size": 0,
                "children": [{"operator_name": "SCAN", "operator_cardinality": 1}],
            },
        }
        (sql / f"{phase}.json").write_text(json.dumps(document))
    report = reporting.index_profiles(tmp_path, {"commands": []})
    assert report["workloads"]["base"][0]["seconds"] == 2
    assert report["workloads"]["batch"][0]["seconds"] == 1
    assert report["profiles"] == 3
    assert report["indexed_profiles"] == 2
    assert report["excluded_phases"] == {"validation": 1}
    assert len((tmp_path / "query-index.jsonl").read_text().splitlines()) == 2
    assert (tmp_path / "queries.parquet").is_file()


def test_offline_index_detects_deleted_native_evidence(tmp_path: Path) -> None:
    event = {
        "event": "sql_profile",
        "profile_path": "/container/sql/missing.json",
        "phase": "base",
    }
    (tmp_path / "events-1.jsonl").write_text(json.dumps(event) + "\n")
    report = reporting.index_profiles(tmp_path, {"commands": []})
    assert report["status"] == "failed"
    assert report["missing_workload_profiles"] == ["missing.json"]
    assert report["indexed_profiles"] == 0
