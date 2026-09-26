"""The retry policy, keyed to the engine's S4.0 exit-code taxonomy.

Pure by construction — no database, no clock reads — so the matrix an operator
meets at the worst possible moment is testable on a bare runner. The rows are
docs/backend-design.md §5's, and the two retryable classes are exactly the two
the engine's S4.7 table marks retryable (``transient_io``, ``lock_conflict``);
the CLI never retries automatically, so retrying is the control plane's job and
nobody else's.
"""

from __future__ import annotations

from dataclasses import dataclass

__all__ = [
    "CANCELED",
    "DISPATCHING",
    "Disposition",
    "FAILED",
    "JOB_KINDS",
    "QUEUED",
    "RUNNING",
    "STATES",
    "SUCCEEDED",
    "SUPERSEDED",
    "dispose",
]

QUEUED = "queued"
DISPATCHING = "dispatching"
RUNNING = "running"
CANCELING = "canceling"
SUCCEEDED = "succeeded"
FAILED = "failed"
CANCELED = "canceled"
SUPERSEDED = "superseded"

STATES: tuple[str, ...] = (
    QUEUED,
    DISPATCHING,
    RUNNING,
    CANCELING,
    SUCCEEDED,
    FAILED,
    CANCELED,
    SUPERSEDED,
)

#: The states in which a job holds its org's one-active slot.
ACTIVE_STATES: tuple[str, ...] = (DISPATCHING, RUNNING, CANCELING)

#: The pipeline work the dispatcher knows how to run (docs/backend-design.md §5).
#: ``train`` is deliberately a separate kind: S4.0 says run-all never trains.
JOB_KINDS: tuple[str, ...] = ("run_all_full", "run_all_incremental", "correct", "train")

#: Fixed backoff for a lock conflict: the dispatcher serializes per org, so a
#: conflict means an out-of-band writer (an operator's CLI run) holds the lock.
LOCK_CONFLICT_DELAY_SECONDS = 30.0

#: Cap on exponential backoff for transient retries.
MAX_DELAY_SECONDS = 300.0


@dataclass(frozen=True)
class Disposition:
    """What the dispatcher does with a finished (or failed-to-run) attempt."""

    #: The job's next state: ``succeeded``, ``failed``, or ``queued`` (retry).
    state: str
    #: ``completed`` | ``no_op`` for successes; ``None`` otherwise.
    outcome: str | None
    #: Whether the retry must re-dispatch with ``--resume <run_id>`` semantics.
    retry_with_resume: bool
    #: Seconds before the retry becomes claimable; 0 for terminal states.
    delay_seconds: float
    #: Whether this attempt counts against ``max_attempts``. A lock conflict
    #: does not: the job never ran.
    consume_attempt: bool


def _retry_delay(attempt: int) -> float:
    return min(MAX_DELAY_SECONDS, 15.0 * (2.0**attempt))


def dispose(
    exit_code: int | None,
    error_class: str | None,
    *,
    attempt: int,
    max_attempts: int,
) -> Disposition:
    """The §5 retry matrix, one row per S4.0 exit / S4.7 class combination.

    ``exit_code is None`` means the runner process died without a status of its
    own — killed, OOM, lost — which is the ``infra`` row: retry with resume,
    because the in-lake ledger knows which stage the run reached.
    """
    if exit_code == 0:
        return Disposition(SUCCEEDED, "completed", False, 0.0, True)
    if exit_code == 10:
        return Disposition(SUCCEEDED, "no_op", False, 0.0, True)
    if exit_code == 2:
        return Disposition(FAILED, None, False, 0.0, True)
    if exit_code == 3:
        if error_class == "lock_conflict":
            return Disposition(QUEUED, None, False, LOCK_CONFLICT_DELAY_SECONDS, False)
        return Disposition(FAILED, None, False, 0.0, True)
    # Exit 1 or a dead process: retryable only for the classes S4.7 marks so,
    # and only while attempts remain.
    retryable = error_class == "transient_io" or exit_code is None
    if retryable and attempt + 1 < max_attempts:
        return Disposition(QUEUED, None, True, _retry_delay(attempt), True)
    return Disposition(FAILED, None, False, 0.0, True)
