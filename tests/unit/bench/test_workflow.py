"""Unit arm of the benchmark workflow (ER-101): the S9.2 dispatch surface, per-scale
runner/timeout, SHA pins, in-image Python, and the compare→upload→teardown ordering.

`benchmarks/workflow.py` is the single parser; these tests read the workflow only through
it. `benchmarks/` is imported the way the image imports it.
"""

from __future__ import annotations

import sys
from pathlib import Path
from types import ModuleType

import pytest

REPO_ROOT = Path(__file__).resolve().parents[3]
WORKFLOW = REPO_ROOT / ".github" / "workflows" / "benchmark.yaml"
BASELINES = REPO_ROOT / "benchmarks" / "baselines"


def _import(module: str) -> ModuleType:
    entry = str(REPO_ROOT / "benchmarks")
    if entry not in sys.path:
        sys.path.insert(0, entry)
    return __import__(module)


workflow = _import("workflow")
scales = _import("scales")


def _dispatchable_scales() -> list[str]:
    loaded = scales.load_scales()
    return [name for name, scale in loaded.items() if scale.dispatchable]


def test_dispatch_options_equal_committed_baselines(tmp_path: Path) -> None:
    """AC2: the dispatch options are exactly the committed baseline stems, and not 1m."""
    options = workflow.parse_benchmark_workflow(WORKFLOW).dispatch_options
    committed = {p.stem for p in BASELINES.glob("*.json")}
    assert options == committed, f"options {set(options)} != committed baselines {committed}"
    assert "1m" not in options

    # A stray option that no baseline backs must make the equality fail.
    strayed = tmp_path / "benchmark.yaml"
    strayed.write_text(
        WORKFLOW.read_text(encoding="utf-8").replace(
            "        options:\n          - smoke\n",
            "        options:\n          - smoke\n          - 1m\n",
        ),
        encoding="utf-8",
    )
    strayed_options = workflow.parse_benchmark_workflow(strayed).dispatch_options
    assert strayed_options != committed, "a stray dispatch option was not detected"
    assert "1m" in strayed_options


def test_runner_and_timeout_match_scales_yaml() -> None:
    """AC3: runner matches each scale's scales.yaml row; timeout is 120 for 100k, else 40."""
    envelope = workflow.parse_benchmark_workflow(WORKFLOW)
    loaded = scales.load_scales()
    for name in _dispatchable_scales():
        assert envelope.runner_for(name) == loaded[name].runner, f"runner for {name}"
        expected_timeout = 120 if name == "100k" else 40
        assert envelope.timeout_for(name) == expected_timeout, f"timeout for {name}"


def test_every_uses_is_sha_pinned(tmp_path: Path) -> None:
    """AC4: every `uses:` is a 40-hex SHA with a `v…` tag comment; a tag pin fails."""
    pins = workflow.parse_benchmark_workflow(WORKFLOW).uses_pins
    assert pins, "the workflow has no `uses:` steps to check"
    for pin in pins:
        assert pin.is_sha_pinned, f"{pin.action} is not SHA-pinned: {pin.sha} # {pin.comment}"

    tagged = tmp_path / "benchmark.yaml"
    tagged.write_text(
        WORKFLOW.read_text(encoding="utf-8").replace(
            "actions/checkout@11bd71901bbe5b1630ceea73d27597364c9af683 # v4.2.2",
            "actions/checkout@v4.2.2 # v4.2.2",
        ),
        encoding="utf-8",
    )
    tagged_pins = workflow.parse_benchmark_workflow(tagged).uses_pins
    assert any(not pin.is_sha_pinned and pin.action == "actions/checkout" for pin in tagged_pins), (
        "a tag-pinned action was not flagged"
    )


def test_no_runner_side_python_toolchain() -> None:
    """AC5: no setup-uv, no `uv sync`, no bare `uv run`; every Python runs in-image."""
    envelope = workflow.parse_benchmark_workflow(WORKFLOW)
    forbidden = ("astral-sh/setup-uv", "uv sync", "uv run")
    for step in envelope.steps:
        blob = f"{step.get('uses', '')}\n{step.get('run', '')}"
        for token in forbidden:
            assert token not in blob, f"step {step.get('name')!r} uses the host toolchain ({token})"
        run = str(step.get("run", ""))
        if "python " in run or "python benchmarks" in run:
            assert "er-pipeline:ci python" in run or "run --rm benchmark" in run, (
                f"step {step.get('name')!r} runs Python outside the image: {run[:80]}"
            )


def test_compare_is_in_image_and_precedes_upload() -> None:
    """AC6: the in-image compare carries the threshold and baselines, and runs before the
    always-upload; teardown is always() too."""
    envelope = workflow.parse_benchmark_workflow(WORKFLOW)
    assert 0 <= envelope.compare_step_index < envelope.upload_step_index
    command = envelope.compare_command
    assert "--baselines-dir /app/benchmarks/baselines" in command
    assert '--scale "$SCALE"' in command
    assert "--fail-threshold 1.25" in command
    assert "run --rm benchmark" in command, "the compare does not run in-image"
    assert envelope.upload_if == "always()"
    assert envelope.upload_if_no_files_found == "error"
    assert envelope.teardown_if == "always()"


def test_triggers_permissions_and_concurrency() -> None:
    """AC7: workflow_dispatch + one weekly cron; contents: read; cancel-in-progress false."""
    envelope = workflow.parse_benchmark_workflow(WORKFLOW)
    assert envelope.scheduled_crons == ("0 6 * * 1",)
    assert envelope.permissions_contents == "read"
    assert envelope.concurrency_cancel_in_progress is False


def test_workflow_has_dispatch_trigger() -> None:
    """The dispatch surface exists (options are parsed), alongside the schedule."""
    envelope = workflow.parse_benchmark_workflow(WORKFLOW)
    assert envelope.dispatch_options, "no workflow_dispatch scale options parsed"
    with pytest.raises(FileNotFoundError):
        workflow.parse_benchmark_workflow(Path("/nonexistent/benchmark.yaml"))
