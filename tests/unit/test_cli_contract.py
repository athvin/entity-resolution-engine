"""Unit tests for the S4.0 CLI contract and the S4.7 taxonomy (S8.4).

Every test here asserts something S4.0, S4.7 or S5.2 states, not something the
implementation happens to do: the command tree and its three global flags, the
class->exit and class->retryable tables, config validation preceding any lake
environment read, the two ``run-all`` chains and their propagation rule, the
NoOpStage/NotImplementedStage split that keeps the S12 M1 exit gate honest, one
stderr JSON line per stage under one ``run_id``, and ``--json`` moving stdout
alone.

Nothing here opens a connection or needs Docker: that is the point of a CLI
skeleton, and it is why these are unit tests rather than integration ones.
"""

from __future__ import annotations

import json
import os
from collections.abc import Iterator
from pathlib import Path
from typing import Any

import pytest
import typer.main
import yaml
from typer.testing import CliRunner

from er import cli
from er.cli import (
    COMMANDS,
    GlobalOptions,
    NoOpStage,
    NotImplementedStage,
    Stage,
    app,
    dbt_vars,
    run_all_chain,
)
from er.config.loader import load_config
from er.dbt_runner import render_dbt_vars
from er.errors import (
    ERROR_CLASS_TO_EXIT,
    RETRYABLE,
    ErrorClass,
    ExitCode,
    StageFailure,
)

REPO_ROOT = Path(__file__).resolve().parents[2]
TEST_CONFIG = REPO_ROOT / "configs" / "test.yaml"

# A literal 26-character ULID, so the "--run-id is threaded verbatim" assertion
# compares against a value the CLI could not have minted itself.
SUPPLIED_RUN_ID = "01JQZ8XKQ4T7VN3M2B9CDEFGHJ"

#: The four stages `er run-all --skip-ingest` chains (S4.0). The M1 exit gate
#: names exactly these four `run_stages` rows.
CHAIN_STAGES = ("standardize", "match", "reconcile", "assemble")

runner = CliRunner()


def stage_lines(stderr: str) -> list[dict[str, Any]]:
    """The S5.2 stage records on stderr, in emission order.

    Filtering to JSON objects rather than counting raw lines is deliberate: a
    usage error also lands on stderr, and it is not a stage.
    """
    records: list[dict[str, Any]] = []
    for line in stderr.splitlines():
        if line.startswith("{"):
            records.append(json.loads(line))
    return records


def invoke(*args: str) -> Any:
    """Run the app with the repo's own S6 document unless one is supplied."""
    argv = list(args)
    if "--config" not in argv:
        argv += ["--config", str(TEST_CONFIG)]
    return runner.invoke(app, argv)


def command_paths() -> dict[str, Any]:
    """The command tree of ``app``, keyed by the path a user types after ``er``."""
    root = typer.main.get_command(app)

    def walk(group: Any, prefix: str) -> Iterator[tuple[str, Any]]:
        for name, command in getattr(group, "commands", {}).items():
            path = f"{prefix}{name}"
            if getattr(command, "commands", None):
                yield from walk(command, f"{path} ")
            else:
                yield path, command

    return dict(walk(root, ""))


@pytest.fixture
def invalid_config(tmp_path: Path) -> Path:
    """The repo's document with ``training.u_seed`` removed — S6.1 V10, exit 2."""
    document = yaml.safe_load(TEST_CONFIG.read_text(encoding="utf-8"))
    del document["training"]["u_seed"]
    path = tmp_path / "invalid.yaml"
    path.write_text(yaml.safe_dump(document, sort_keys=False), encoding="utf-8")
    return path


def test_command_tree_matches_s4_0() -> None:
    """AC1: the tree is exactly S4.0's command table, flags and all."""
    commands = command_paths()
    assert set(commands) == set(COMMANDS)

    for path, command in commands.items():
        flags = {flag for param in command.params for flag in param.opts}
        missing = {"--config", "--run-id", "--json"} - flags
        assert not missing, f"`er {path}` does not accept {sorted(missing)}"


def test_error_class_exit_and_retryable_maps() -> None:
    """AC2: the S4.7 table, both columns, total over ErrorClass."""
    assert ERROR_CLASS_TO_EXIT == {
        ErrorClass.TRANSIENT_IO: ExitCode.STAGE_FAILURE,
        ErrorClass.LOCK_CONFLICT: ExitCode.PRECONDITION,
        ErrorClass.PRECONDITION: ExitCode.PRECONDITION,
        ErrorClass.CONFIG: ExitCode.CONFIG,
        ErrorClass.CONTRADICTION: ExitCode.STAGE_FAILURE,
        ErrorClass.NON_CONVERGENCE: ExitCode.STAGE_FAILURE,
        ErrorClass.DATA: ExitCode.STAGE_FAILURE,
    }
    assert {int(code) for code in ExitCode} == {0, 1, 2, 3, 10}
    assert {error_class for error_class in ErrorClass if RETRYABLE[error_class]} == {
        ErrorClass.TRANSIENT_IO,
        ErrorClass.LOCK_CONFLICT,
    }


class RecordingEnviron(dict[str, str]):
    """An ``os.environ`` stand-in that remembers every name looked up."""

    def __init__(self, values: dict[str, str]) -> None:
        super().__init__(values)
        self.reads: list[str] = []

    def __getitem__(self, key: str) -> str:
        self.reads.append(key)
        return super().__getitem__(key)

    def get(self, key: str, default: Any = None) -> Any:
        self.reads.append(key)
        return super().get(key, default)

    def __contains__(self, key: object) -> bool:
        self.reads.append(str(key))
        return super().__contains__(key)


@pytest.mark.parametrize("command", ["run-all --mode incremental --skip-ingest", "standardize"])
def test_invalid_config_exits_2_before_touching_lake_env(
    command: str, invalid_config: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """AC3: a document that fails S6 validation exits 2, naming the pointer, and
    the process never reaches a lake environment variable."""
    environ = RecordingEnviron({})
    monkeypatch.setattr(os, "environ", environ)

    result = runner.invoke(app, [*command.split(), "--config", str(invalid_config)])

    assert result.exit_code == int(ExitCode.CONFIG)
    assert "/training/u_seed" in result.stderr
    assert stage_lines(result.stderr) == []
    lake_reads = [
        name for name in environ.reads if name == "ER_CATALOG_DSN" or name.startswith("ER_S3_")
    ]
    assert lake_reads == []


def test_run_all_requires_source_or_skip_ingest() -> None:
    """AC4: neither --source/--path nor --skip-ingest is exit 2 and zero stages."""
    result = invoke("run-all", "--mode", "incremental")

    assert result.exit_code == int(ExitCode.CONFIG)
    assert stage_lines(result.stderr) == []


def test_run_all_chain_order_per_mode() -> None:
    """AC4: both S4.0 chains, in order, with the flags each stage carries."""
    assert [(stage.name, stage.args) for stage in run_all_chain("incremental", True)] == [
        ("standardize", ("--changed-only",)),
        ("match", ("--mode", "incremental")),
        ("reconcile", ()),
        ("assemble", ("--touched-only",)),
    ]
    assert [(stage.name, stage.args) for stage in run_all_chain("full", True)] == [
        ("standardize", ()),
        ("match", ("--mode", "full")),
        ("reconcile", ()),
        ("assemble", ()),
    ]
    with_ingest = run_all_chain("incremental", False, source="crm", path="/app/drop/crm")
    assert with_ingest[0].name == "ingest"
    assert with_ingest[0].args == ("--source", "crm", "--path", "/app/drop/crm")
    assert [stage.name for stage in with_ingest[1:]] == list(CHAIN_STAGES)

    with pytest.raises(ValueError, match="unknown run-all mode"):
        run_all_chain("sideways", True)


def test_ten_does_not_abort_chain_and_first_failure_propagates(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    """AC5: a chain of 10s exits 0; the first real failure stops the chain."""
    all_no_op = invoke("run-all", "--mode", "incremental", "--skip-ingest")

    assert all_no_op.exit_code == int(ExitCode.SUCCESS)
    records = stage_lines(all_no_op.stderr)
    assert [record["stage"] for record in records] == list(CHAIN_STAGES)
    assert [record["exit_code"] for record in records] == [int(ExitCode.NOTHING_TO_DO)] * 4

    def failing_chain(mode: str, skip_ingest: bool, **kwargs: Any) -> list[Stage]:
        stages = run_all_chain(mode, skip_ingest, **kwargs)
        stages[1] = FailingStage(name=stages[1].name)
        return stages

    monkeypatch.setattr(cli, "run_all_chain", failing_chain)
    failed = invoke("run-all", "--mode", "incremental", "--skip-ingest")

    assert failed.exit_code == int(ExitCode.STAGE_FAILURE)
    assert [record["stage"] for record in stage_lines(failed.stderr)] == ["standardize", "match"]


class FailingStage:
    """A stage that fails the way a real one does — by raising, not by returning."""

    def __init__(self, name: str) -> None:
        self.name = name
        self.args: tuple[str, ...] = ()

    def run(self, options: GlobalOptions) -> int:
        raise StageFailure(f"{self.name} blew up")


def test_not_implemented_stage_is_exit_1() -> None:
    """AC6: an unwritten stage is exit 1 with its message, and never in the chain.

    ``er review`` is the example because it is still a stub. ``er train`` was until
    ER-055 implemented it and ``er assert`` until ER-062 did; a test that keeps
    asserting a stub over an implemented stage stops testing the split it is named
    for.
    """
    result = invoke("review", "list")

    assert result.exit_code == int(ExitCode.STAGE_FAILURE)
    assert "stage not implemented: review" in result.stderr
    record = stage_lines(result.stderr)[0]
    assert record["status"] == "failed"
    assert record["error_class"] == ErrorClass.DATA.value
    assert record["exit_code"] not in (int(ExitCode.SUCCESS), int(ExitCode.NOTHING_TO_DO))

    for mode in ("incremental", "full"):
        for skip_ingest in (True, False):
            chain = run_all_chain(mode, skip_ingest, source="crm", path="/app/drop/crm")
            assert all(isinstance(stage, NoOpStage) for stage in chain)
            assert not any(isinstance(stage, NotImplementedStage) for stage in chain)


def test_run_id_is_minted_once_and_threaded() -> None:
    """AC7: one JSON line per stage, all four under a single run_id."""
    supplied = invoke(
        "run-all", "--mode", "incremental", "--skip-ingest", "--run-id", SUPPLIED_RUN_ID
    )
    records = stage_lines(supplied.stderr)
    assert len(records) == len(CHAIN_STAGES)
    assert {record["run_id"] for record in records} == {SUPPLIED_RUN_ID}

    minted = invoke("run-all", "--mode", "incremental", "--skip-ingest")
    minted_records = stage_lines(minted.stderr)
    minted_ids = {record["run_id"] for record in minted_records}
    assert len(minted_records) == len(CHAIN_STAGES)
    assert len(minted_ids) == 1
    assert len(minted_ids.pop()) == 26

    second = invoke("run-all", "--mode", "incremental", "--skip-ingest")
    assert stage_lines(second.stderr)[0]["run_id"] != minted_records[0]["run_id"]


def test_json_flag_switches_stdout_only() -> None:
    """AC8: --json changes stdout to JSONL and leaves stderr untouched."""
    human = invoke("run-all", "--mode", "incremental", "--skip-ingest")
    machine = invoke("run-all", "--mode", "incremental", "--skip-ingest", "--json")

    assert human.exit_code == machine.exit_code == int(ExitCode.SUCCESS)
    assert len(stage_lines(machine.stderr)) == len(stage_lines(human.stderr))
    assert len(machine.stderr.splitlines()) == len(human.stderr.splitlines())

    machine_stdout = machine.stdout.splitlines()
    # One line per stage plus the final run summary (S4.0 stdout column).
    assert len(machine_stdout) == len(CHAIN_STAGES) + 1
    assert len(machine_stdout) == len(human.stdout.splitlines())
    for line in machine_stdout:
        json.loads(line)
    with pytest.raises(json.JSONDecodeError):
        json.loads(human.stdout.splitlines()[0])


def test_dbt_vars_key_set() -> None:
    """AC8: the dbt --vars payload is exactly the S6 keys, from the document.

    ER-033 moved the builder to ``er.dbt_runner.render_dbt_vars`` and widened the
    payload with the two vars S4.2 needs — the ``sources`` column mapping and the
    ``standardization`` block — so this asserts the wider payload and that the CLI
    exports THE builder rather than a second copy of it.
    """
    config = load_config(TEST_CONFIG)
    assert dbt_vars is render_dbt_vars

    variables = dbt_vars(config, SUPPLIED_RUN_ID)
    assert variables == {
        "std_version": config.versions.std_version,
        "survivorship_version": config.versions.survivorship_version,
        "run_id": SUPPLIED_RUN_ID,
        "sources": {name: source.model_dump() for name, source in config.sources.items()},
        "standardization": config.standardization.model_dump(),
    }
