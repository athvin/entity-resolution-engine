"""The S5.2 stage record: one JSON line on stderr, with a fixed key set (S8.4).

Asserted through the CLI rather than against :func:`~er.obs.logging.emit_stage_record`
alone, because "exactly one line per stage" is a claim about what an invocation
writes to the stream, not about what one function does when called once.
"""

from __future__ import annotations

import io
import json
from pathlib import Path
from typing import Any

import pytest
from typer.testing import CliRunner

from er.cli import app, emit_stage_line
from er.errors import ErrorClass, ExitCode
from er.obs.logging import STAGE_RECORD_KEYS, emit_stage_record

REPO_ROOT = Path(__file__).resolve().parents[3]
TEST_CONFIG = REPO_ROOT / "configs" / "test.yaml"

# A literal 26-character ULID, so "the record carries the run's id" compares against
# a value the CLI could not have minted for itself.
RUN_ID = "01JQZ8XKQ4T7VN3M2B9CDEFGHJ"

# The S5.2 example line's keys, in its order. `exit_code` is deliberately absent:
# the example does not carry it, and the test below states exactly how the shipped
# key set differs from it.
S5_2_KEYS = (
    "run_id",
    "stage",
    "seq",
    "status",
    "started_at",
    "ended_at",
    "snapshot_start",
    "snapshot_end",
    "config_hash",
    "model_version",
    "tf_snapshot_id",
    "rows_in",
    "rows_out",
    "candidate_pairs",
    "pairs_above_auto_merge",
    "entities_created",
    "entities_merged",
    "entities_split",
    "entities_retired",
    "edges_cut",
    "review_queue_added",
    "duration_ms",
    "error_class",
    "error_detail",
)

runner = CliRunner()


def invoke(*args: str) -> Any:
    """Run the app against the repo's own S6 document, under a fixed run id."""
    return runner.invoke(app, [*args, "--config", str(TEST_CONFIG), "--run-id", RUN_ID])


def lines(stderr: str) -> list[str]:
    return [line for line in stderr.splitlines() if line.strip()]


def test_exactly_one_json_line_with_exact_key_set() -> None:
    """AC2: one stage, one line, whose key set is exactly STAGE_RECORD_KEYS."""
    result = invoke("standardize")

    assert result.exit_code == int(ExitCode.NOTHING_TO_DO)
    emitted = lines(result.stderr)
    assert len(emitted) == 1, f"a stage emitted more than its one S5.2 line:\n{result.stderr}"

    record = json.loads(emitted[0])
    assert tuple(record) == STAGE_RECORD_KEYS
    assert set(record) - set(STAGE_RECORD_KEYS) == set()
    assert set(STAGE_RECORD_KEYS) - set(record) == set()
    assert record["run_id"] == RUN_ID
    assert record["stage"] == "standardize"
    assert record["seq"] == 1
    assert record["status"] == "succeeded"
    assert record["exit_code"] == int(ExitCode.NOTHING_TO_DO)
    assert record["error_class"] is None
    assert record["duration_ms"] >= 0


def test_key_set_is_s5_2_plus_exit_code() -> None:
    """AC2: every S5.2 key is present, in order; `exit_code` is the one addition.

    S4.0 makes exit `10` a *successful* no-op, so `status` alone cannot tell a stage
    that did its work from one that found none — which is why ER-014 put the code on
    the record and why `tests/unit/test_cli_contract.py` asserts it.
    """
    assert tuple(key for key in STAGE_RECORD_KEYS if key != "exit_code") == S5_2_KEYS
    assert set(STAGE_RECORD_KEYS) - set(S5_2_KEYS) == {"exit_code"}


def test_failed_stage_line_carries_error_class() -> None:
    """AC2/AC6: a failed stage emits the same line, classified, and still one of it.

    ``er review`` is the example because it is still an unimplemented stub, and a stub
    fails without a lake. ``er train`` was until ER-055 implemented it and ``er assert``
    until ER-062 did.
    """
    result = invoke("review", "list")

    assert result.exit_code == int(ExitCode.STAGE_FAILURE)
    emitted = lines(result.stderr)
    assert len(emitted) == 1

    record = json.loads(emitted[0])
    assert tuple(record) == STAGE_RECORD_KEYS
    assert record["status"] == "failed"
    assert record["error_class"] == ErrorClass.DATA.value
    assert record["error_detail"] == "stage not implemented: review"
    assert record["ended_at"] is not None


def test_emitter_refuses_a_record_that_is_not_the_s5_2_key_set() -> None:
    """AC2: the key set is enforced where it is written, not only where it is read."""
    complete = dict.fromkeys(STAGE_RECORD_KEYS)
    stream = io.StringIO()

    emit_stage_record(complete, stream=stream)
    assert len(stream.getvalue().splitlines()) == 1

    with pytest.raises(ValueError, match="missing"):
        emit_stage_record({key: None for key in STAGE_RECORD_KEYS[:-1]}, stream=io.StringIO())
    with pytest.raises(ValueError, match="extra"):
        emit_stage_record({**complete, "surprise": 1}, stream=io.StringIO())


def test_cli_exports_the_one_emitter() -> None:
    """AC2: `emit_stage_line` is THE emitter under its ER-014 name, not a second one."""
    assert emit_stage_line is emit_stage_record
