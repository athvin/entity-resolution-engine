"""The S4.0 config-drift decision, as a pure function (DesignDoc.md S4.0, S5.1).

S4.0: `er run-all --mode incremental` "refuses to proceed (exit `3`) when
`(config_hash, model_version, std_version)` differ from the last successful run for
this tenant; `--allow-escalate` promotes the run to `--mode full` instead of failing".
S5.1 adds the fourth field: changing `versions.std_version` or
`versions.survivorship_version` "invalidates the derived corpus … A version bump is
exactly the drift the S4.0 config-drift guard catches".

The whole point of :func:`~er.versions.check_mode_preconditions` being pure is this
file: the drift matrix is the part an operator meets when a run refuses, and it is
asserted here against plain dataclasses, on the bare runner S8.1 gives the unit layer
— no lake, no catalog, no object store, no config file. The arms that need a lake
(what a refusal leaves in `runs`, what an escalation records) are
`tests/integration/test_mode_guard.py`'s.
"""

from __future__ import annotations

import itertools
import os

import pytest

from er.lake.model import RUN_MODES, RUN_STATUSES
from er.versions import (
    ESCALATION_MESSAGE,
    FINGERPRINT_FIELDS,
    MODE_FULL,
    MODE_INCREMENTAL,
    REFUSAL_MESSAGE,
    STATUS_SUCCEEDED,
    UNSET_VALUE,
    ModeOutcome,
    RunFingerprint,
    check_mode_preconditions,
)

#: The baseline every case below drifts away from. Values that are distinguishable by
#: eye, so a message asserting "both values" cannot pass on a coincidence.
BASELINE = RunFingerprint(
    config_hash="a" * 64,
    model_version="v0001",
    std_version="1",
    survivorship_version="1",
)

#: What each field becomes when it is the one that drifted.
DRIFTED: dict[str, str] = {
    "config_hash": "b" * 64,
    "model_version": "v0002",
    "std_version": "2",
    "survivorship_version": "2",
}


def drift(**changes: str | None) -> RunFingerprint:
    """:data:`BASELINE` with ``changes`` applied — the current run's fingerprint.

    Constructed field by field rather than splatted: `model_version` is the only
    nullable one (S5), so a `None` anywhere else is a typo in the case rather than a
    fingerprint worth comparing.
    """
    values: dict[str, str | None] = {name: BASELINE.value(name) for name in FINGERPRINT_FIELDS}
    values.update(changes)
    config_hash, std_version = values["config_hash"], values["std_version"]
    survivorship_version = values["survivorship_version"]
    if config_hash is None or std_version is None or survivorship_version is None:
        raise ValueError("only model_version is nullable in a RunFingerprint")
    return RunFingerprint(
        config_hash=config_hash,
        model_version=values["model_version"],
        std_version=std_version,
        survivorship_version=survivorship_version,
    )


def test_the_fingerprint_is_the_four_runs_columns() -> None:
    """S4.0's three fields plus S5.1's fourth, and nothing else."""
    assert FINGERPRINT_FIELDS == (
        "config_hash",
        "model_version",
        "std_version",
        "survivorship_version",
    )
    # The vocabulary this module spells for itself rather than importing back from
    # `er.obs.runctx`, which imports `code_version` from `er.versions` (see the
    # constant's own note). A divergence from S5 fails here, not as a predicate that
    # silently matches no row.
    assert STATUS_SUCCEEDED in RUN_STATUSES
    assert {MODE_INCREMENTAL, MODE_FULL} <= RUN_MODES


@pytest.mark.parametrize("field", FINGERPRINT_FIELDS)
def test_drift_matrix_over_the_four_fields(field: str) -> None:
    """AC3, first half: each of the four drifting ALONE refuses with exit 3.

    Parametrised over the tuple rather than over four hand-written cases, so a field
    added to the fingerprint and not to the guard's comparison fails here.
    """
    current = drift(**{field: DRIFTED[field]})

    decision = check_mode_preconditions(BASELINE, current, MODE_INCREMENTAL)

    assert decision.outcome is ModeOutcome.REFUSE
    assert decision.refused is True
    assert decision.mode == MODE_INCREMENTAL, "a refusal must not also change the mode"
    assert [item.field for item in decision.drift] == [field]
    # "a message identifying which of the four fields drifted and both values".
    assert field in decision.message
    assert str(BASELINE.value(field)) in decision.message
    assert DRIFTED[field] in decision.message
    for other in FINGERPRINT_FIELDS:
        if other != field:
            assert other not in decision.message


@pytest.mark.parametrize(
    "unchanged",
    [
        pytest.param(subset, id="+".join(subset) or "none")
        for size in range(len(FINGERPRINT_FIELDS) + 1)
        for subset in itertools.combinations(FINGERPRINT_FIELDS, size)
    ],
)
def test_no_combination_of_unchanged_fields_refuses(unchanged: tuple[str, ...]) -> None:
    """AC3, second half: re-stating any subset of the baseline is never drift.

    Every subset, including the empty one and the whole tuple, because "unchanged" has
    to mean unchanged however many fields are involved — a guard that fired on a
    two-field identity would refuse every second run.
    """
    current = drift(**{name: BASELINE.value(name) for name in unchanged})

    decision = check_mode_preconditions(BASELINE, current, MODE_INCREMENTAL)

    assert decision.outcome is ModeOutcome.PROCEED
    assert decision.drift == ()
    assert decision.message == ""
    assert decision.mode == MODE_INCREMENTAL


@pytest.mark.parametrize("field", FINGERPRINT_FIELDS)
def test_allow_escalate_promotes_every_drift_to_full(field: str) -> None:
    """AC2's decision half: the same drift escalates rather than refusing."""
    current = drift(**{field: DRIFTED[field]})

    decision = check_mode_preconditions(BASELINE, current, MODE_INCREMENTAL, allow_escalate=True)

    assert decision.outcome is ModeOutcome.ESCALATE
    assert decision.escalated is True
    assert decision.refused is False
    # The promotion is to `full` and it is the value the caller builds its chain from:
    # an escalated run that still returned `incremental` would rebuild nothing.
    assert decision.mode == MODE_FULL
    assert [item.field for item in decision.drift] == [field]
    assert decision.message.startswith("--allow-escalate")


def test_allow_escalate_does_not_promote_a_run_that_did_not_drift() -> None:
    """`--allow-escalate` is a fallback, not a mode switch.

    S4.0 makes it promote "instead of failing", so with nothing to fail on there is
    nothing to promote — a flag that escalated unconditionally would turn every
    scheduled incremental run into a full rebuild.
    """
    decision = check_mode_preconditions(BASELINE, BASELINE, MODE_INCREMENTAL, allow_escalate=True)

    assert decision.outcome is ModeOutcome.PROCEED
    assert decision.mode == MODE_INCREMENTAL


def test_a_first_run_cannot_drift() -> None:
    """AC4's decision half: no prior successful run means no baseline to differ from."""
    decision = check_mode_preconditions(None, BASELINE, MODE_INCREMENTAL)

    assert decision.outcome is ModeOutcome.PROCEED
    assert decision.drift == ()


@pytest.mark.parametrize("allow_escalate", [False, True])
def test_full_mode_is_never_guarded(allow_escalate: bool) -> None:
    """AC5's decision half: all four fields drifted, and `--mode full` proceeds.

    A full run recomputes the derived corpus S5.1 says the bump invalidated, so there
    is nothing left for the guard to protect.
    """
    current = drift(**DRIFTED)

    decision = check_mode_preconditions(BASELINE, current, MODE_FULL, allow_escalate=allow_escalate)

    assert decision.outcome is ModeOutcome.PROCEED
    assert decision.mode == MODE_FULL
    assert decision.drift == ()


def test_a_null_model_version_renders_as_unset_on_both_sides() -> None:
    """`model_version` is the one nullable field (S5), and NULL is a value that moved.

    A run recorded before any model was activated carries NULL; the run after it
    carries `v0001`. That is drift, and the message has to be readable — an empty
    string beside `last=` would read as "the value is blank".
    """
    prior = drift(model_version=None)

    decision = check_mode_preconditions(prior, BASELINE, MODE_INCREMENTAL)

    assert decision.refused is True
    assert f"model_version last={UNSET_VALUE} now=v0001" in decision.message


def test_an_unknown_mode_is_refused_rather_than_guessed() -> None:
    """Neither `incremental` nor `full` is a caller bug, not a third policy."""
    with pytest.raises(ValueError, match="unknown run-all mode"):
        check_mode_preconditions(BASELINE, BASELINE, "correction_pass")


def test_the_fingerprint_refuses_a_field_it_does_not_have() -> None:
    """A mistyped field name must not compare two `None`s and call them equal."""
    with pytest.raises(ValueError, match="not a fingerprint field"):
        BASELINE.value("confighash")


def test_decision_function_is_pure(monkeypatch: pytest.MonkeyPatch) -> None:
    """AC6: no lake, no catalog, no environment — and no mutation of its arguments.

    The two doors out of this function are replaced by raisers rather than counted by
    a spy: a spy that recorded a call would still have let the guard reach the
    substrate, and the claim S4.0 rests on is that the decision is reachable when
    nothing else is. Every `ER_*` variable is removed for the same reason — this is
    the bare runner S8.1 gives the unit layer, made bare on purpose.
    """

    def refuse_connection(*args: object, **kwargs: object) -> object:
        raise AssertionError("check_mode_preconditions opened a DuckDB connection")

    def refuse_read(*args: object, **kwargs: object) -> object:
        raise AssertionError("check_mode_preconditions read the runs relation")

    monkeypatch.setattr("duckdb.connect", refuse_connection)
    monkeypatch.setattr("er.versions.last_successful_run", refuse_read)
    for name in [name for name in os.environ if name.startswith("ER_")]:
        monkeypatch.delenv(name)

    prior = BASELINE
    current = drift(config_hash=DRIFTED["config_hash"])
    first = check_mode_preconditions(prior, current, MODE_INCREMENTAL)
    second = check_mode_preconditions(prior, current, MODE_INCREMENTAL)

    # Same inputs, same decision: the function holds no state between calls, so a run
    # cannot be refused because an earlier one was.
    assert first == second
    assert prior == BASELINE, "the guard mutated the baseline it was given"
    assert current == drift(config_hash=DRIFTED["config_hash"])
    assert first.message == REFUSAL_MESSAGE.format(drift=str(first.drift[0]))
    assert check_mode_preconditions(
        prior, current, MODE_INCREMENTAL, allow_escalate=True
    ).message == ESCALATION_MESSAGE.format(drift=str(first.drift[0]))
