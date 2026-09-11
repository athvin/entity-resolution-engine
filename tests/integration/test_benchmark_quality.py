"""ER-100 integration: a smoke run carries the quality block, and quality is reported but
never gated (S10.5, M21). One real measured pass is run via `report.py --run` and both
arms read its output.

`report.main --run` manages its own lake connection (it runs `er init` and connects), so
this module deliberately does not take the harness `initialised_lake` fixture; the T-INV-1
finalizer skips when no harness lake was attached. `benchmarks/` is imported as the image
imports it.
"""

from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path
from types import ModuleType
from typing import Any, Final

import pytest

from er.dbt_runner import DBT_PROFILES_DIR, DBT_PROJECT_DIR

REPO_ROOT: Final = Path(__file__).resolve().parents[2]

SPEC_TEST_IDS: Final[tuple[str, ...]] = (
    "tests/integration/test_benchmark_quality.py::test_quality_block_is_reported_not_gated",
)


def _import(module: str) -> ModuleType:
    entry = str(REPO_ROOT / "benchmarks")
    if entry not in sys.path:
        sys.path.insert(0, entry)
    return __import__(module)


report = _import("report")
scales = _import("scales")


@pytest.fixture(scope="module")
def dbt_packages() -> None:
    if (Path(DBT_PROJECT_DIR) / "dbt_packages" / "dbt_utils").is_dir():
        return
    completed = subprocess.run(
        ["dbt", "deps", "--project-dir", DBT_PROJECT_DIR, "--profiles-dir", DBT_PROFILES_DIR],
        capture_output=True,
        text=True,
        check=False,
    )
    assert completed.returncode == 0, completed.stdout + completed.stderr


@pytest.fixture(scope="module")
def smoke_run(
    dbt_packages: None, tmp_path_factory: pytest.TempPathFactory
) -> tuple[int, dict[str, Any]]:
    """One real `report.py --run` smoke pass; returns (exit_code, run document)."""
    out = tmp_path_factory.mktemp("quality") / "latest.json"
    code = report.main(["--run", "--scale", "smoke", "--repeat", "1", "--out", str(out)])
    document = json.loads(out.read_text(encoding="utf-8"))
    return code, document


def test_blocking_recall_present_in_latest_json(smoke_run: tuple[int, dict[str, Any]]) -> None:
    """AC2, AC7: the run document carries blocking_recall and the six-number quality block,
    and the schema rejects a document missing blocking_recall."""
    _, document = smoke_run
    schema = _import("schema")

    assert "blocking_recall" in document, "blocking_recall is a required top-level key (S8.5)"
    assert isinstance(document["blocking_recall"], (int, float))
    assert set(document["quality"]) == {
        "edge_precision",
        "edge_recall",
        "edge_f1",
        "cluster_precision",
        "cluster_recall",
        "cluster_f1",
    }
    schema.validate_bench_result(document)

    without = {k: v for k, v in document.items() if k != "blocking_recall"}
    with pytest.raises(schema.BenchResultError):
        schema.validate_bench_result(without)


def test_quality_block_is_reported_not_gated(smoke_run: tuple[int, dict[str, Any]]) -> None:
    """AC5: degrading quality changes neither the verdict nor the exit code — a comparable
    smoke run with no baseline is NO_BASELINE (exit 0) regardless of its quality numbers."""
    code, document = smoke_run
    scale = scales.get_scale("smoke")

    # The run is comparable (measured inside the smoke envelope) and has no baseline here.
    assert document["verdict"] == "NO_BASELINE", document["verdict"]
    assert code == 0

    verdict_before, _ = report.compare_to_baseline(document, None, scale)

    degraded = json.loads(json.dumps(document))
    degraded["quality"]["cluster_recall"] = 0.5
    degraded["quality"]["cluster_f1"] = 0.5
    degraded["blocking_recall"] = 0.5
    verdict_after, _ = report.compare_to_baseline(degraded, None, scale)

    assert verdict_after is verdict_before, "a quality drop changed the verdict — it must not"
    assert verdict_after.exit_code == 0, "a NO_BASELINE run must exit 0 whatever its quality"
