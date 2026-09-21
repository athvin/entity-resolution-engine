import gzip
import importlib
import json
import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[3] / "benchmarks"))

performance = importlib.import_module("performance")
compare_outputs = performance.compare_outputs
detailed_metrics = performance.detailed_metrics
processing_resources = performance.processing_resources
variation = performance.variation


def result(directory: Path, probability: float = 0.99) -> Path:
    directory.mkdir()
    fingerprint = dict.fromkeys(
        (
            "duckdb_version",
            "splink_version",
            "dbt_core_version",
            "dbt_duckdb_version",
            "ducklake_extension_version",
            "cgroup_cpu_max",
            "cgroup_memory_max",
            "er_duckdb_threads",
            "er_duckdb_memory_limit",
        ),
        "same",
    )
    document = {
        "input_sha256": {"file": "same"},
        "semantic_config_sha256": "same",
        "fingerprint": fingerprint,
    }
    for phase in ("base", "batch"):
        document.update(
            {
                f"{phase}_counts": {"records": 2},
                f"{phase}_partition_hash": "same",
                f"{phase}_semantic_hashes": {"golden": "same"},
            }
        )
        with gzip.open(directory / f"{phase}-scores.json.gz", "wt") as handle:
            json.dump([["a", "b", probability, "hash-a", "hash-b", True]], handle)
    path = directory / "result.json"
    path.write_text(json.dumps(document))
    return path


def test_comparison_allows_only_tiny_probability_roundoff(tmp_path: Path) -> None:
    first = result(tmp_path / "baseline")
    second = result(tmp_path / "candidate", 0.99 + 1e-12)
    assert compare_outputs(first, second)["matching"]
    third = result(tmp_path / "wrong", 0.99 + 1e-7)
    assert not compare_outputs(first, third)["matching"]


def test_equal_counts_cannot_hide_changed_values_or_environment(tmp_path: Path) -> None:
    first = result(tmp_path / "baseline")
    second = result(tmp_path / "candidate")
    document = json.loads(second.read_text())
    document["batch_semantic_hashes"]["golden"] = "changed winner"
    document["fingerprint"]["er_duckdb_threads"] = "extra CPU"
    second.write_text(json.dumps(document))
    comparison = compare_outputs(first, second)
    assert not comparison["matching"]
    assert set(comparison["differences"]) == {
        "batch_semantic_hashes",
        "fingerprint.er_duckdb_threads",
    }


def test_variation_detects_noisy_comparison() -> None:
    assert variation([100, 100, 100]) == 0
    assert variation([80, 100, 120]) > 0.05


def test_resource_totals_exclude_setup_and_idle_samples(tmp_path: Path) -> None:
    samples = [
        {
            "monotonic_ns": tick,
            "usage_usec": tick * 1_000_000,
            "throttled_usec": tick * 100_000,
            "memory.current": memory,
        }
        for tick, memory in ((0, 9999), (2, 100), (3, 150), (6, 9999), (8, 200), (9, 300))
    ]
    (tmp_path / "resources.jsonl").write_text("\n".join(json.dumps(s) for s in samples))
    run = {
        "commands": [
            {"phase": "setup", "started_ns": 0, "ended_ns": 1, "command": ["er", "init"]},
            {"phase": "base", "started_ns": 2, "ended_ns": 4, "command": ["er", "ingest"]},
            {"phase": "batch", "started_ns": 8, "ended_ns": 10, "command": ["er", "run-all"]},
        ]
    }
    measured = processing_resources(tmp_path / "result.json", run)
    assert measured["cpu_s"] == 2
    assert measured["throttled_s"] == pytest.approx(0.2)
    assert measured["sampled_memory_peak_bytes"] == 300
    run["commands"][1]["ended_ns"] = 2
    with pytest.raises(AssertionError, match="CPU sampling unavailable"):
        processing_resources(tmp_path / "result.json", run)
    for entry in run["commands"]:
        entry["cpu"] = {"usage_usec": 3_000_000, "throttled_usec": 200_000}
    measured = processing_resources(tmp_path / "result.json", run)
    assert measured["cpu_s"] == 6
    assert measured["throttled_s"] == pytest.approx(0.4)


def test_diagnostics_exclude_validation_reusing_the_last_invocation_id(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    import profile_report

    def event(tick: int) -> dict:
        return {
            "name": "lake.open",
            "phase": "base",
            "invocation_id": "run",
            "monotonic_ns": tick,
            "duration_ms": 1,
        }

    summary = {
        "run": {
            "commands": [
                {"phase": "base", "invocation_id": "run", "started_ns": 10, "ended_ns": 20}
            ]
        },
        "spans": [event(15), event(25)],
        "queries": [event(15), event(25)],
    }
    monkeypatch.setattr(profile_report, "summarize", lambda _: summary)
    metrics = detailed_metrics(tmp_path / "result.json")
    assert metrics["connection_opens"] == 1
    assert metrics["sql_profiles"] == 1
