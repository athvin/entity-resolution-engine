"""The runner executes a job payload through :mod:`er.service` in-process.

Stage bodies are the same no-op injection the CLI contract tests use, so this
runs on a bare runner and asserts the seam: payload in, S4.0 chain executed
under one run_id, terminal result record out.
"""

from __future__ import annotations

import json
from pathlib import Path

import pytest
from erserver import runner

from er import cli as cli_module
from er.cli import Stage

REPO_ROOT = Path(__file__).resolve().parents[2]
TEST_CONFIG = REPO_ROOT / "configs" / "test.yaml"

RUN_ID = "01JQZ8XKQ4T7VN3M2B9CDEFGHJ"


@pytest.fixture(autouse=True)
def isolated_stage_bodies(monkeypatch: pytest.MonkeyPatch) -> None:
    original = cli_module._stage_for

    def factory(name: str, args: tuple[str, ...] = ()) -> Stage:
        if name in {"ingest", "standardize", "match", "reconcile", "assemble"}:
            return cli_module.NoOpStage(name=name, args=tuple(args))
        return original(name, args)

    monkeypatch.setattr(cli_module, "_stage_for", factory)


def payload(kind: str = "run_all_incremental") -> dict[str, object]:
    return {
        "job_id": "job-1",
        "kind": kind,
        "params": {"skip_ingest": True},
        "config_path": str(TEST_CONFIG),
        "run_id": RUN_ID,
    }


def test_execute_runs_the_chain_and_reports_the_run() -> None:
    record = runner.execute(payload())

    assert record["job_id"] == "job-1"
    assert record["run_id"] == RUN_ID
    assert record["mode"] == "incremental"
    assert record["exit_code"] == 0
    assert [stage["stage"] for stage in record["stages"]] == [
        "standardize",
        "match",
        "reconcile",
        "assemble",
    ]


def test_main_prints_one_result_line_and_exits_with_the_status(
    capsys: pytest.CaptureFixture[str],
) -> None:
    status = runner.main(["erserver.runner", json.dumps(payload())])

    out = capsys.readouterr().out.strip().splitlines()
    record = json.loads(out[-1])
    assert status == 0
    assert record["exit_code"] == 0
    assert record["run_id"] == RUN_ID


def test_unknown_kind_is_a_caller_bug() -> None:
    with pytest.raises(ValueError, match="unknown job kind"):
        runner.execute(payload(kind="sideways"))
