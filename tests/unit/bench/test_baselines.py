"""Unit arm of ER-102: the committed baselines, the S9.1 `--validate-baselines` coupling
lint, and the two benchmark-workflow edits (dispatch options + scheduled scale).

`benchmarks/` is imported the way the image imports it (like `test_workflow.py`), and the
workflow is read only through `benchmarks/workflow.py` — never a second YAML grep. The
baseline/dispatch coupling is the one S12/M5 makes a static-job gate: a dispatchable scale
with no committed baseline, a baseline measured outside its S10.2 envelope, or a preflight
that hardcodes the envelope instead of sourcing it from `scales.yaml` each fail here rather
than on a weekly run no one watches.
"""

from __future__ import annotations

import shutil
import sys
from pathlib import Path
from types import ModuleType

import yaml

REPO_ROOT = Path(__file__).resolve().parents[3]
WORKFLOW = REPO_ROOT / ".github" / "workflows" / "benchmark.yaml"
CI_WORKFLOW = REPO_ROOT / ".github" / "workflows" / "ci.yaml"
BASELINES = REPO_ROOT / "benchmarks" / "baselines"
README = BASELINES / "README.md"


def _import(module: str) -> ModuleType:
    entry = str(REPO_ROOT / "benchmarks")
    if entry not in sys.path:
        sys.path.insert(0, entry)
    return __import__(module)


report = _import("report")
scales = _import("scales")
schema = _import("schema")


def _copy_baselines(dest: Path) -> Path:
    dest.mkdir(parents=True, exist_ok=True)
    for path in BASELINES.glob("*.json"):
        shutil.copy(path, dest / path.name)
    return dest


def test_validate_baselines_accepts_committed_tree() -> None:
    """AC1: on the committed tree, the baseline set, the dispatch options and every
    dispatchable scale's envelope agree — `--validate-baselines` finds nothing."""
    assert report.validate_baselines(BASELINES, WORKFLOW) == []


def test_validate_baselines_rejects_missing_baseline(tmp_path: Path) -> None:
    """AC1: a dispatch option with no committed baseline is a violation naming the scale."""
    baselines = _copy_baselines(tmp_path / "baselines")
    (baselines / "10k.json").unlink()
    violations = report.validate_baselines(baselines, WORKFLOW)
    assert violations, "removing 10k.json must be rejected"
    assert any("10k" in line for line in violations), violations


def test_validate_baselines_rejects_runner_envelope_mismatch(tmp_path: Path) -> None:
    """AC2: a workflow whose 100k runner is downgraded, and one whose preflight exports a
    field other than mem_limit, are each rejected naming the offending scale/field."""
    runner_workflow = tmp_path / "runner.yaml"
    runner_workflow.write_text(
        WORKFLOW.read_text(encoding="utf-8").replace("ubuntu-latest-8-cores", "ubuntu-latest"),
        encoding="utf-8",
    )
    runner_violations = report.validate_baselines(BASELINES, runner_workflow)
    assert any("100k" in line and "runner" in line for line in runner_violations), runner_violations

    field_workflow = tmp_path / "field.yaml"
    field_workflow.write_text(
        WORKFLOW.read_text(encoding="utf-8").replace(
            "--field mem_limit", "--field duckdb_memory_limit"
        ),
        encoding="utf-8",
    )
    field_violations = report.validate_baselines(BASELINES, field_workflow)
    assert any("mem_limit" in line and "ER_MEM_LIMIT" in line for line in field_violations), (
        field_violations
    )


def test_committed_baselines_are_comparable_under_s10_4() -> None:
    """AC3: every committed baseline is comparable under its own S10.2 row — scale matches
    the filename, the cgroup/DuckDB envelope matches scales.yaml, repeat >= 3, every phase
    wall_ms CV <= 0.15, and the verdict is not NON_COMPARABLE."""
    for path in sorted(BASELINES.glob("*.json")):
        baseline = report._load_json(path)
        scale = scales.get_scale(path.stem)
        assert str(baseline["fingerprint"]["scale"]) == path.stem, path.name
        assert report.comparability_violations(baseline, scale) == [], path.name
        assert int(baseline["repeat"]) >= 3, path.name
        assert baseline["verdict"] != report.Verdict.NON_COMPARABLE.value, path.name
        for phase in baseline["phases"]:
            assert float(phase["wall_ms_cv"]) <= report.CV_CEILING, (path.name, phase["name"])


def test_committed_baselines_match_result_schema_and_quality_keys() -> None:
    """AC4: every committed baseline validates against the result schema and carries
    non-null incremental_ratio and blocking_recall."""
    for path in sorted(BASELINES.glob("*.json")):
        baseline = report._load_json(path)
        schema.validate_bench_result(baseline)
        assert baseline.get("incremental_ratio") is not None, path.name
        assert baseline.get("blocking_recall") is not None, path.name


def test_scheduled_scale_is_10k_everywhere() -> None:
    """AC5: dispatch options are {smoke,10k,100k}, the scheduled scale is 10k, and no
    `inputs.scale || 'smoke'` fallback survives anywhere in the workflow text."""
    envelope = report.parse_benchmark_workflow(WORKFLOW)
    assert set(envelope.dispatch_options) == {"smoke", "10k", "100k"}
    assert envelope.scheduled_scale == "10k"
    assert "inputs.scale || 'smoke'" not in WORKFLOW.read_text(encoding="utf-8")


def test_ci_static_job_has_baseline_dispatch_parity_step() -> None:
    """AC6: ci.yaml's static job runs the Baseline/dispatch parity step with the three
    flags S9.1 shows, and that is the only --validate-baselines invocation in either
    workflow."""
    document = yaml.safe_load(CI_WORKFLOW.read_text(encoding="utf-8"))
    static_steps = document["jobs"]["static"]["steps"]
    parity = [step for step in static_steps if step.get("name") == "Baseline/dispatch parity"]
    assert len(parity) == 1, "the static job must carry exactly one Baseline/dispatch parity step"
    command = str(parity[0]["run"])
    assert "benchmarks/report.py --validate-baselines" in command
    assert "--baselines-dir benchmarks/baselines" in command
    assert "--workflow .github/workflows/benchmark.yaml" in command

    # An invocation is a `run:` step that calls it, not a comment mentioning it, so count
    # the parsed run commands across both workflows rather than raw occurrences.
    def _run_invocations(path: Path) -> int:
        document = yaml.safe_load(path.read_text(encoding="utf-8"))
        return sum(
            str(step.get("run", "")).count("--validate-baselines")
            for job in document["jobs"].values()
            for step in job.get("steps", [])
        )

    assert _run_invocations(CI_WORKFLOW) + _run_invocations(WORKFLOW) == 1, (
        "exactly one --validate-baselines invocation across the workflows"
    )


def test_baselines_readme_documents_envelopes() -> None:
    """AC7: the README restates each committed baseline's four S10.2 envelope values and
    carries the literal --write-baseline bootstrap command."""
    text = README.read_text(encoding="utf-8")
    assert "--write-baseline" in text
    for path in sorted(BASELINES.glob("*.json")):
        scale = scales.get_scale(path.stem)
        for value in (
            scale.runner,
            str(scale.cpu_limit),
            scale.mem_limit,
            scale.duckdb_memory_limit,
        ):
            assert value in text, f"{path.stem}: README omits envelope value {value!r}"
