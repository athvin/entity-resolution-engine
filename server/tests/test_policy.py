"""The §5 retry matrix, row by row. Pure: no database, no clock."""

from __future__ import annotations

import pytest
from erserver.policy import FAILED, JOB_KINDS, QUEUED, SCHEDULABLE_KINDS, SUCCEEDED, dispose


def test_success_and_no_op_are_both_terminal_successes() -> None:
    done = dispose(0, None, attempt=0, max_attempts=3)
    assert (done.state, done.outcome) == (SUCCEEDED, "completed")

    no_op = dispose(10, None, attempt=0, max_attempts=3)
    assert (no_op.state, no_op.outcome) == (SUCCEEDED, "no_op")


def test_config_failure_is_permanent() -> None:
    verdict = dispose(2, "config", attempt=0, max_attempts=3)
    assert verdict.state == FAILED
    assert not verdict.retry_with_resume


def test_transient_io_retries_with_resume_until_attempts_run_out() -> None:
    first = dispose(1, "transient_io", attempt=0, max_attempts=3)
    assert first.state == QUEUED
    assert first.retry_with_resume
    assert first.consume_attempt
    assert first.delay_seconds > 0

    second = dispose(1, "transient_io", attempt=1, max_attempts=3)
    assert second.state == QUEUED
    assert second.delay_seconds > first.delay_seconds

    exhausted = dispose(1, "transient_io", attempt=2, max_attempts=3)
    assert exhausted.state == FAILED


def test_other_stage_failures_do_not_retry() -> None:
    verdict = dispose(1, "data", attempt=0, max_attempts=3)
    assert verdict.state == FAILED


def test_lock_conflict_requeues_without_consuming_an_attempt() -> None:
    verdict = dispose(3, "lock_conflict", attempt=0, max_attempts=3)
    assert verdict.state == QUEUED
    assert not verdict.consume_attempt
    assert not verdict.retry_with_resume
    assert verdict.delay_seconds > 0


def test_other_preconditions_fail_permanently() -> None:
    verdict = dispose(3, "precondition", attempt=0, max_attempts=3)
    assert verdict.state == FAILED


def test_a_dead_runner_is_infra_and_retries_with_resume() -> None:
    verdict = dispose(None, "infra", attempt=0, max_attempts=3)
    assert verdict.state == QUEUED
    assert verdict.retry_with_resume


@pytest.mark.parametrize("attempt", [2, 5])
def test_infra_retries_also_respect_max_attempts(attempt: int) -> None:
    assert dispose(None, "infra", attempt=attempt, max_attempts=3).state == FAILED


def test_provision_is_a_kind_but_never_schedulable() -> None:
    assert "provision" in JOB_KINDS
    assert "provision" not in SCHEDULABLE_KINDS
    assert set(SCHEDULABLE_KINDS) < set(JOB_KINDS)
