"""stdout is command output; stderr is telemetry (S4 preamble, S5.2, S8.4).

The property both tests here assert is the one a caller depends on: it can pipe
stdout — with or without ``--json`` — and the S5.2 records keep arriving on stderr,
one per stage, neither moved nor duplicated.
"""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any

import pytest
from typer.testing import CliRunner

from er.cli import app
from er.errors import ExitCode
from er.obs.logging import STAGE_RECORD_KEYS

REPO_ROOT = Path(__file__).resolve().parents[3]
TEST_CONFIG = REPO_ROOT / "configs" / "test.yaml"

#: The four stages `er run-all --skip-ingest` chains (S4.0).
CHAIN_STAGES = ("standardize", "match", "reconcile", "assemble")

runner = CliRunner()


def invoke(*args: str) -> Any:
    return runner.invoke(app, [*args, "--config", str(TEST_CONFIG)])


def stage_records(stderr: str) -> list[dict[str, Any]]:
    """Every S5.2 record on stderr, in emission order."""
    return [json.loads(line) for line in stderr.splitlines() if line.startswith("{")]


def test_human_mode_stdout_is_not_json() -> None:
    """AC3: in human mode NO stdout line parses as JSON, and stderr is unaffected."""
    result = invoke("run-all", "--mode", "incremental", "--skip-ingest")

    assert result.exit_code == int(ExitCode.SUCCESS)
    stdout_lines = result.stdout.splitlines()
    # One line per stage plus the final run summary (S4.0's stdout column).
    assert len(stdout_lines) == len(CHAIN_STAGES) + 1
    for line in stdout_lines:
        with pytest.raises(json.JSONDecodeError):
            json.loads(line)

    records = stage_records(result.stderr)
    assert [record["stage"] for record in records] == list(CHAIN_STAGES)
    assert len(result.stderr.splitlines()) == len(CHAIN_STAGES)


def test_json_mode_moves_stdout_only() -> None:
    """AC3: --json makes every stdout line JSON and leaves the stderr lines alone."""
    human = invoke("run-all", "--mode", "incremental", "--skip-ingest")
    machine = invoke("run-all", "--mode", "incremental", "--skip-ingest", "--json")

    assert human.exit_code == machine.exit_code == int(ExitCode.SUCCESS)
    for line in machine.stdout.splitlines():
        json.loads(line)
    assert len(machine.stdout.splitlines()) == len(human.stdout.splitlines())

    # Counted on both streams for the same run, which is what makes "--json never
    # moves the stderr line" an assertion rather than an inspection.
    human_records = stage_records(human.stderr)
    machine_records = stage_records(machine.stderr)
    assert len(machine_records) == len(human_records) == len(CHAIN_STAGES)
    assert len(machine.stderr.splitlines()) == len(human.stderr.splitlines())
    for record in machine_records:
        assert tuple(record) == STAGE_RECORD_KEYS
    assert [record["seq"] for record in machine_records] == [1, 2, 3, 4]
    assert len({record["run_id"] for record in machine_records}) == 1
