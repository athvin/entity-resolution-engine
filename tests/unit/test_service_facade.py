"""Unit tests for :mod:`er.service`, the in-process orchestration facade.

Everything here runs on a bare runner with no services, exactly as the S8.4
CLI contract tests do: the writer lock, the correction gate and the schema
preflight all pass through on a missing lake environment, and the real chain
stages report ``10`` (nothing to do). What is asserted is the facade's own
contract — the CLI's sequence with a typed outcome instead of a process exit,
the S4.0 propagation rule, and refusals that keep their S4.7 class.
"""

from __future__ import annotations

from pathlib import Path
from typing import Any

import pytest
import yaml

from er import service
from er.cli import GlobalOptions, Stage, run_all_chain
from er.errors import ErrorClass, ExitCode, StageFailure

REPO_ROOT = Path(__file__).resolve().parents[2]
TEST_CONFIG = REPO_ROOT / "configs" / "test.yaml"

SUPPLIED_RUN_ID = "01JQZ8XKQ4T7VN3M2B9CDEFGHJ"

CHAIN_STAGES = ("standardize", "match", "reconcile", "assemble")


class FailingStage:
    """A stage that fails the way a real one does — by raising, not returning."""

    def __init__(self, name: str) -> None:
        self.name = name
        self.args: tuple[str, ...] = ()

    def run(self, options: GlobalOptions) -> int:
        raise StageFailure(f"{self.name} blew up")


@pytest.fixture(autouse=True)
def isolated_stage_bodies(monkeypatch: pytest.MonkeyPatch) -> None:
    """These facade tests inject no-op bodies, exactly as the CLI contract tests do."""
    from er import cli as cli_module

    original = cli_module._stage_for

    def factory(name: str, args: tuple[str, ...] = ()) -> Stage:
        if name in {"ingest", "standardize", "match", "reconcile", "assemble"}:
            return cli_module.NoOpStage(name=name, args=tuple(args))
        return original(name, args)

    monkeypatch.setattr(cli_module, "_stage_for", factory)


def test_no_op_chain_returns_success_with_all_stage_outcomes() -> None:
    outcome = service.run_pipeline(
        mode="incremental",
        config_path=TEST_CONFIG,
        run_id=SUPPLIED_RUN_ID,
        skip_ingest=True,
    )

    assert outcome.run_id == SUPPLIED_RUN_ID
    assert outcome.mode == "incremental"
    assert outcome.exit_code == int(ExitCode.SUCCESS)
    assert outcome.error_class is None
    assert [stage.stage for stage in outcome.stages] == list(CHAIN_STAGES)
    assert {stage.exit_code for stage in outcome.stages} == {int(ExitCode.NOTHING_TO_DO)}


def test_first_failure_stops_the_chain_and_carries_its_class(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    def failing_chain(mode: str, skip_ingest: bool, **kwargs: Any) -> list[Stage]:
        stages = run_all_chain(mode, skip_ingest, **kwargs)
        stages[1] = FailingStage(name=stages[1].name)
        return stages

    monkeypatch.setattr(service, "run_all_chain", failing_chain)
    outcome = service.run_pipeline(mode="incremental", config_path=TEST_CONFIG, skip_ingest=True)

    assert outcome.exit_code == int(ExitCode.STAGE_FAILURE)
    assert [stage.stage for stage in outcome.stages] == ["standardize", "match"]
    assert outcome.stages[-1].error_class is not None
    assert outcome.error_class == outcome.stages[-1].error_class
    assert outcome.error_detail == "match blew up"


def test_invalid_config_refuses_with_exit_2_and_config_semantics(tmp_path: Path) -> None:
    document = yaml.safe_load(TEST_CONFIG.read_text(encoding="utf-8"))
    del document["training"]["u_seed"]
    invalid = tmp_path / "invalid.yaml"
    invalid.write_text(yaml.safe_dump(document, sort_keys=False), encoding="utf-8")

    outcome = service.run_pipeline(mode="incremental", config_path=invalid, skip_ingest=True)

    assert outcome.exit_code == int(ExitCode.CONFIG)
    assert outcome.stages == ()
    assert outcome.error_detail is not None


def test_missing_source_and_path_refuses_as_config_before_any_stage() -> None:
    outcome = service.run_pipeline(mode="full", config_path=TEST_CONFIG)

    assert outcome.exit_code == int(ExitCode.CONFIG)
    assert outcome.error_class == ErrorClass.CONFIG.value
    assert outcome.stages == ()


def test_unknown_rebuild_reason_refuses_as_config() -> None:
    outcome = service.run_pipeline(
        mode="full", config_path=TEST_CONFIG, skip_ingest=True, reason="sideways"
    )

    assert outcome.exit_code == int(ExitCode.CONFIG)
    assert outcome.stages == ()


def test_unknown_mode_is_a_caller_bug_not_a_refusal() -> None:
    with pytest.raises(ValueError, match="unknown run-all mode"):
        service.run_pipeline(mode="sideways", config_path=TEST_CONFIG, skip_ingest=True)


def test_correction_run_id_resume_disagreement_is_a_caller_bug() -> None:
    with pytest.raises(ValueError, match="run_id must equal resume"):
        service.run_correction(
            config_path=TEST_CONFIG, run_id=SUPPLIED_RUN_ID, resume="01JQZ8XKQ4T7VN3M2B9CDEFGHK"
        )


def test_training_on_a_bare_runner_refuses_rather_than_raises() -> None:
    outcome = service.run_training(config_path=TEST_CONFIG, run_id=SUPPLIED_RUN_ID)

    assert outcome.run_id == SUPPLIED_RUN_ID
    assert outcome.mode == "train"
    assert outcome.exit_code == int(ExitCode.CONFIG)
    assert "ERR_ENV_MISSING" in (outcome.error_detail or "")


def test_correction_on_a_bare_runner_refuses_rather_than_raises() -> None:
    outcome = service.run_correction(config_path=TEST_CONFIG, run_id=SUPPLIED_RUN_ID)

    assert outcome.run_id == SUPPLIED_RUN_ID
    assert outcome.mode == "correction_pass"
    assert outcome.exit_code in (int(ExitCode.CONFIG), int(ExitCode.PRECONDITION))
    assert outcome.stages == ()
