"""`--resume`'s decision, without a lake (DesignDoc.md S4.7, S8.4).

Every refusal here is one an operator meets while a run is already broken, which is
the worst possible moment to discover that the guard needed a Compose stack to be
exercised. :func:`~er.resume.resume_plan` is a pure function of rows and a hash for
exactly that reason, and these are the tests that spend it.
"""

from __future__ import annotations

import pytest

from er.errors import ConfigError, ErrorClass, ExitCode, PreconditionFailure
from er.resume import ResumeRow, resume_plan

RUN_ID = "01JQZ8XKQ4T7VN3M2B9CDEFGHJ"
CONFIG_HASH = "a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f90"
DRIFTED_HASH = "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"
MODEL_VERSION = "v0003"

#: The four stages `er run-all --skip-ingest` records (S4.0), and the shape of the
#: ledger a mid-run failure at `match` leaves behind (S4.7).
FAILED_AT_MATCH = (("standardize", "succeeded"), ("match", "failed"))


def ledger(
    stages: tuple[tuple[str, str], ...],
    *,
    run_status: str = "failed",
    config_hash: str = CONFIG_HASH,
) -> tuple[ResumeRow, ...]:
    """The `runs`-joined `run_stages` rows for a run of ``stages``."""
    return tuple(
        ResumeRow(
            run_id=RUN_ID,
            mode="incremental",
            run_status=run_status,
            config_hash=config_hash,
            model_version=MODEL_VERSION,
            stage=stage,
            seq=seq,
            status=status,
        )
        for seq, (stage, status) in enumerate(stages, start=1)
    )


def test_first_non_succeeded_stage_is_selected() -> None:
    """AC5: the plan restarts at stage k and pins the run's own provenance."""
    plan = resume_plan(ledger(FAILED_AT_MATCH), CONFIG_HASH)

    assert plan.resume_from == "match"
    assert plan.completed == ("standardize",)
    # S4.7: "with the original run_id, config_hash and model_version". None of the
    # three is re-derived, and nothing is minted.
    assert plan.run_id == RUN_ID
    assert plan.config_hash == CONFIG_HASH
    assert plan.model_version == MODEL_VERSION
    assert plan.mode == "incremental"

    # `seq` is what orders the ledger, not the order the rows arrived in.
    shuffled = tuple(reversed(ledger(FAILED_AT_MATCH)))
    assert resume_plan(shuffled, CONFIG_HASH) == plan

    # "the FIRST stage whose status is not succeeded": a stage still `running` is not
    # succeeded either, and a later failure does not displace an earlier unfinished one.
    interrupted = ledger(
        (("standardize", "succeeded"), ("match", "running"), ("reconcile", "failed"))
    )
    assert resume_plan(interrupted, CONFIG_HASH).resume_from == "match"


def test_config_hash_mismatch_is_exit_2() -> None:
    """AC6: a config that moved under the run is refused, and both hashes are named."""
    with pytest.raises(ConfigError) as refusal:
        resume_plan(ledger(FAILED_AT_MATCH), DRIFTED_HASH)

    assert refusal.value.code == int(ExitCode.CONFIG)
    assert refusal.value.error_class is ErrorClass.CONFIG
    assert CONFIG_HASH in str(refusal.value)
    assert DRIFTED_HASH in str(refusal.value)

    # Checked before the status guard: a run that both drifted and succeeded is
    # reported as the drift, because that is the fault an operator has to fix first.
    with pytest.raises(ConfigError):
        resume_plan(ledger((("standardize", "succeeded"),), run_status="succeeded"), DRIFTED_HASH)


def test_already_succeeded_run_is_exit_3() -> None:
    """AC6: there is nothing to resume in a run that finished."""
    finished = ledger(
        (("standardize", "succeeded"), ("match", "succeeded")), run_status="succeeded"
    )

    with pytest.raises(PreconditionFailure) as refusal:
        resume_plan(finished, CONFIG_HASH)

    assert refusal.value.code == int(ExitCode.PRECONDITION)
    assert refusal.value.error_class is ErrorClass.PRECONDITION
    assert RUN_ID in str(refusal.value)


def test_a_run_with_nothing_recorded_is_exit_3() -> None:
    """An unknown `run_id`, and a run that died before its first stage row, agree."""
    with pytest.raises(PreconditionFailure) as refusal:
        resume_plan((), CONFIG_HASH)
    assert refusal.value.code == int(ExitCode.PRECONDITION)

    # `runs.status='failed'` with every recorded stage `succeeded`: the run died
    # between its last stage and its own terminal write. There is no stage to
    # re-execute, and this module will not guess at the chain to find one.
    with pytest.raises(PreconditionFailure, match="no unfinished stage"):
        resume_plan(ledger((("standardize", "succeeded"),)), CONFIG_HASH)


def test_rows_from_more_than_one_run_are_refused() -> None:
    """A plan built from two runs would restart one of them under the other's id."""
    mixed = (
        *ledger(FAILED_AT_MATCH),
        ResumeRow(
            run_id="01JOTHERRUNIDXXXXXXXXXXXXX",
            mode="incremental",
            run_status="failed",
            config_hash=CONFIG_HASH,
            model_version=MODEL_VERSION,
            stage="reconcile",
            seq=3,
            status="failed",
        ),
    )

    with pytest.raises(PreconditionFailure, match="more than one run"):
        resume_plan(mixed, CONFIG_HASH)

    # S5.2 keys `run_stages` on `(run_id, stage)`; two rows for one stage means the
    # key is already broken and the resume would write over the wrong row.
    duplicated = ledger((("standardize", "succeeded"), ("standardize", "failed")))
    with pytest.raises(PreconditionFailure, match="more than one row for a stage"):
        resume_plan(duplicated, CONFIG_HASH)
