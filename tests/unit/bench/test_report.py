"""Unit arm of `benchmarks/report.py` (ER-099): aggregation, verdicts, comparability and
the exit-code contract (S10.3, S10.4). No lake — the `--run` measurement path is exercised
in the image by ER-101; everything tested here is pure or `--compare`.

`benchmarks/` is a scripts directory, imported the way the image imports it.
"""

from __future__ import annotations

import json
import math
import statistics
import sys
from pathlib import Path
from types import ModuleType
from typing import Any

import pytest

REPO_ROOT = Path(__file__).resolve().parents[3]
DATA_DIR = Path(__file__).resolve().parent / "data"


def _import(module: str) -> ModuleType:
    entry = str(REPO_ROOT / "benchmarks")
    if entry not in sys.path:
        sys.path.insert(0, entry)
    return __import__(module)


report = _import("report")
scales = _import("scales")

PHASES = ("ingest", "standardize", "train", "full_match_reconcile", "assemble", "incremental_cycle")


def _fingerprint(scale_name: str = "smoke") -> dict[str, Any]:
    """A fingerprint that matches the scale's envelope — a comparable run by default."""
    scale = scales.get_scale(scale_name)
    return {
        "scale": scale_name,
        "image_digest": "sha256:test",
        "git_sha": "abc1234",
        "runner": "local",
        "cgroup_cpu_max": str(float(scale.cpu_limit)),
        "cgroup_memory_max": scales._memory_bytes(scale.mem_limit, scale_name, "mem_limit"),
        "cgroup_memory_peak": 1,
        "nproc": 8,
        "er_duckdb_threads": scale.cpu_limit,
        "er_duckdb_memory_limit": scale.duckdb_memory_limit,
        "duckdb_version": "1.5.5",
        "splink_version": "4.0.16",
        "dbt_core_version": "1.12.2",
        "dbt_duckdb_version": "1.11.0",
        "ducklake_extension_version": "0.3",
        "config_hash": "deadbeef",
        "generator_seed": 42,
        "model_version": "v0001",
        "tf_snapshot_id": "01SNAP",
    }


def _result(
    wall_by_phase: dict[str, float] | None = None,
    *,
    scale: str = "smoke",
    cv: float = 0.0,
    fingerprint_overrides: dict[str, Any] | None = None,
) -> dict[str, Any]:
    """A schema-valid single result with the given per-phase wall_ms (100 by default)."""
    walls = wall_by_phase or dict.fromkeys(PHASES, 100.0)
    fingerprint = _fingerprint(scale)
    fingerprint.update(fingerprint_overrides or {})
    return {
        "scale": scale,
        "verdict": report.Verdict.NO_BASELINE.value,
        "repeat": 3,
        "incremental_ratio": 0.5,
        "blocking_recall": 0.99,
        "fingerprint": fingerprint,
        "phases": [
            {
                "name": name,
                "wall_ms": walls[name],
                "wall_ms_cv": cv,
                "records_per_sec": 500.0,
                "candidate_pair_count": 10,
                "pairs_above_auto_merge": 3,
                "memory_peak_bytes": 1024,
                "snapshot_count": 2,
            }
            for name in PHASES
        ],
        "memory": {"duckdb_buffer_peak_bytes": 1, "rss_peak_bytes": 2, "cgroup_peak_bytes": 3},
        "quality": {
            "edge_precision": 1.0,
            "edge_recall": 1.0,
            "edge_f1": 1.0,
            "cluster_precision": 1.0,
            "cluster_recall": 1.0,
            "cluster_f1": 1.0,
        },
    }


def _single_pass(wall_first_phase: float) -> dict[str, Any]:
    walls = dict.fromkeys(PHASES, 100.0)
    walls["ingest"] = wall_first_phase
    return _result(walls)


def test_median_and_cv_across_repeats() -> None:
    """AC2: median 200 and CV = stdev/mean for wall_ms samples 100, 200, 300."""
    passes = [_single_pass(100.0), _single_pass(200.0), _single_pass(300.0)]
    aggregated = report.aggregate_passes(passes)
    ingest = next(p for p in aggregated["phases"] if p["name"] == "ingest")
    assert ingest["wall_ms"] == 200.0
    expected_cv = statistics.stdev([100.0, 200.0, 300.0]) / statistics.mean([100.0, 200.0, 300.0])
    assert math.isclose(ingest["wall_ms_cv"], expected_cv, abs_tol=1e-9)
    assert aggregated["repeat"] == 3


def test_no_baseline_verdict_exits_zero(tmp_path: Path, capsys: Any) -> None:
    """AC3: --compare with no baseline for the scale -> NO_BASELINE, exit 0, in the JSON."""
    run = tmp_path / "run.json"
    out = tmp_path / "out.json"
    run.write_text(json.dumps(_result()), encoding="utf-8")
    code = report.main(
        [
            "--compare",
            str(run),
            "--baselines-dir",
            str(tmp_path / "empty"),
            "--scale",
            "smoke",
            "--out",
            str(out),
        ]
    )
    assert code == 0
    assert capsys.readouterr().out.strip().splitlines()[-1] == "NO_BASELINE"
    assert json.loads(out.read_text())["verdict"] == "NO_BASELINE"


def test_regression_threshold_boundary(tmp_path: Path) -> None:
    """AC4: exactly 1.25x baseline is OK; just above is REGRESSION naming the phase."""
    baselines = tmp_path / "baselines"
    baselines.mkdir()
    (baselines / "smoke.json").write_text(
        json.dumps(_result(dict.fromkeys(PHASES, 100.0))), "utf-8"
    )
    scale = scales.get_scale("smoke")

    at_boundary = _result({**dict.fromkeys(PHASES, 100.0), "ingest": 125.0})
    verdict, _ = report.compare_to_baseline(
        at_boundary, _result(dict.fromkeys(PHASES, 100.0)), scale, fail_threshold=1.25
    )
    assert verdict is report.Verdict.OK

    over = _result({**dict.fromkeys(PHASES, 100.0), "ingest": 125.001})
    over_json = tmp_path / "over.json"
    over_json.write_text(json.dumps(over), "utf-8")
    code = report.main(
        ["--compare", str(over_json), "--baselines-dir", str(baselines), "--scale", "smoke"]
    )
    assert code == 1
    verdict2, reasons = report.compare_to_baseline(
        over, _result(dict.fromkeys(PHASES, 100.0)), scale
    )
    assert verdict2 is report.Verdict.REGRESSION
    assert any("ingest" in reason for reason in reasons)


def test_non_comparable_conditions(tmp_path: Path) -> None:
    """AC5: any single S10.4 condition -> NON_COMPARABLE and exit 3."""
    scale = scales.get_scale("smoke")
    perturbations: list[dict[str, Any]] = [
        {"scale": "10k"},  # scale mismatch
        {"cgroup_cpu_max": "4.0"},  # quota != cpu_limit
        {"cgroup_memory_max": 123},  # memory.max != mem_limit
        {"er_duckdb_threads": 7},  # threads != cpu_limit
        {"er_duckdb_memory_limit": "8GB"},  # != duckdb_memory_limit
    ]
    for override in perturbations:
        run = _result(fingerprint_overrides=override)
        assert report.comparability_violations(run, scale), f"{override} was not flagged"
        verdict, _ = report.compare_to_baseline(run, _result(), scale)
        assert verdict is report.Verdict.NON_COMPARABLE, override

    # A noisy phase (CV above the ceiling) is equally non-comparable.
    noisy = _result(cv=0.2)
    assert report.comparability_violations(noisy, scale)

    run_json = tmp_path / "nc.json"
    run_json.write_text(json.dumps(_result(fingerprint_overrides={"scale": "10k"})), "utf-8")
    code = report.main(["--compare", str(run_json), "--scale", "smoke"])
    assert code == 3


def test_write_baseline_refuses_non_comparable(tmp_path: Path) -> None:
    """AC5: write_baseline refuses a NON_COMPARABLE run and leaves the dir byte-unchanged."""
    baselines = tmp_path / "baselines"
    baselines.mkdir()
    scale = scales.get_scale("smoke")
    non_comparable = _result(fingerprint_overrides={"cgroup_cpu_max": "4.0"})

    with pytest.raises(
        report.BenchResultError if hasattr(report, "BenchResultError") else Exception
    ):
        report.write_baseline(non_comparable, baselines, scale)
    assert list(baselines.iterdir()) == []

    run_json = tmp_path / "nc.json"
    run_json.write_text(json.dumps(non_comparable), "utf-8")
    code = report.main(
        [
            "--compare",
            str(run_json),
            "--scale",
            "smoke",
            "--baselines-dir",
            str(baselines),
            "--write-baseline",
        ]
    )
    assert code != 0
    assert list(baselines.iterdir()) == []


def test_bad_arguments_exit_2(tmp_path: Path) -> None:
    """AC6: a missing --scale, an unreadable run, and invalid JSON all exit 2."""
    assert report.main(["--run"]) == 2  # no --scale

    assert report.main(["--compare", str(tmp_path / "nope.json"), "--scale", "smoke"]) == 2

    bad = tmp_path / "bad.json"
    bad.write_text("{not valid json", encoding="utf-8")
    assert report.main(["--compare", str(bad), "--scale", "smoke"]) == 2


def test_report_md_and_verdict_line(tmp_path: Path, capsys: Any) -> None:
    """AC7: --out writes the JSON and a sibling report.md (one row per phase); the verdict
    is the last line of stdout."""
    run = tmp_path / "run.json"
    out = tmp_path / "sub" / "out.json"
    out.parent.mkdir()
    run.write_text(json.dumps(_result()), encoding="utf-8")
    code = report.main(
        [
            "--compare",
            str(run),
            "--scale",
            "smoke",
            "--baselines-dir",
            str(tmp_path / "none"),
            "--out",
            str(out),
        ]
    )
    assert code == 0
    report_md = (out.parent / "report.md").read_text(encoding="utf-8")
    rows = [
        line for line in report_md.splitlines() if line.startswith("| ") and "phase |" not in line
    ]
    # one header-separator row plus one row per phase
    assert sum(1 for r in rows if not set(r) <= set("|- ")) == len(PHASES)
    assert capsys.readouterr().out.strip().splitlines()[-1] == "NO_BASELINE"


def test_committed_baseline_fixture_is_comparable() -> None:
    """The committed smoke baseline under data/ is itself a comparable, schema-valid run."""
    schema = _import("schema")
    baseline = json.loads((DATA_DIR / "smoke_baseline.json").read_text(encoding="utf-8"))
    schema.validate_bench_result(baseline)
    assert report.comparability_violations(baseline, scales.get_scale("smoke")) == []
