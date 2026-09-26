"""The Postgres-backed job queue: enqueue, claim, transition, inspect.

``SELECT … FOR UPDATE SKIP LOCKED`` with two partial unique indexes
(:mod:`erserver.db`) is the whole coordination story — no broker. Per-tenant
serialization is transactional: a second dispatcher racing for the same org
hits the ``jobs_one_active_per_org`` unique index and backs off, so double
dispatch is impossible by construction rather than by discipline
(docs/backend-design.md §5).

Every function owns its transaction: commit on success, rollback on the
constraint races it expects. The connection is the caller's.
"""

from __future__ import annotations

import json
from dataclasses import dataclass
from typing import Any

import psycopg
from psycopg import errors as pg_errors
from psycopg.rows import dict_row
from psycopg.types.json import Jsonb
from ulid import ULID

from erserver.policy import (
    ACTIVE_STATES,
    CANCELED,
    CANCELING,
    DISPATCHING,
    FAILED,
    JOB_KINDS,
    QUEUED,
    RUNNING,
    Disposition,
)

__all__ = [
    "Job",
    "OrgNotActiveError",
    "UnknownKindError",
    "UnknownOrgError",
    "cancel",
    "claim",
    "enqueue",
    "ensure_org",
    "finish",
    "get_job",
    "heartbeat",
    "job_state",
    "list_jobs",
    "mark_canceled",
    "mark_running",
    "org_row",
    "org_state",
    "parse_result_line",
    "reap_stale",
    "resume_job",
    "set_org_state",
]


class UnknownOrgError(LookupError):
    """The org named by the request does not exist."""


class OrgNotActiveError(RuntimeError):
    """The org exists but is not accepting jobs in its current state."""


class UnknownKindError(ValueError):
    """The job kind is not one the dispatcher knows how to run."""


@dataclass(frozen=True)
class Job:
    """One ``jobs`` row, exactly as stored."""

    job_id: str
    org: str
    kind: str
    params: dict[str, Any]
    state: str
    priority: int
    idempotency_key: str | None
    run_id: str | None
    attempt: int
    max_attempts: int
    exit_code: int | None
    error_class: str | None
    error_detail: str | None
    outcome: str | None
    progress: dict[str, Any]


_JOB_COLUMNS = (
    "job_id, org, kind, params, state, priority, idempotency_key, run_id, "
    "attempt, max_attempts, exit_code, error_class, error_detail, outcome, progress"
)


def _job(row: dict[str, Any]) -> Job:
    return Job(**row)


def ensure_org(
    connection: psycopg.Connection,
    name: str,
    *,
    config_path: str,
    env: dict[str, str] | None = None,
    drop_root: str | None = None,
    state: str | None = None,
) -> None:
    """Create or update the org row the dispatcher reads at launch time.

    ``state`` applies only on insert — a replayed create must never knock an
    active org back to ``provisioning`` — and defaults to ``active`` for the
    manually provisioned orgs that predate the lifecycle.
    """
    with connection.cursor() as cursor:
        cursor.execute(
            "INSERT INTO orgs (name, config_path, env, drop_root, state) "
            "VALUES (%s, %s, %s, %s, coalesce(%s, 'active')) "
            "ON CONFLICT (name) DO UPDATE SET config_path = EXCLUDED.config_path, "
            "env = EXCLUDED.env, drop_root = EXCLUDED.drop_root",
            (name, config_path, Jsonb(env or {}), drop_root, state),
        )
    connection.commit()


def org_row(connection: psycopg.Connection, name: str) -> tuple[str, dict[str, str]] | None:
    """The org's ``(config_path, env)`` or ``None`` when it does not exist."""
    with connection.cursor() as cursor:
        cursor.execute("SELECT config_path, env FROM orgs WHERE name = %s", (name,))
        row = cursor.fetchone()
    return None if row is None else (row[0], row[1])


def org_record(connection: psycopg.Connection, name: str) -> dict[str, Any] | None:
    """The full org row the API works from, or ``None``."""
    with connection.cursor(row_factory=dict_row) as cursor:
        cursor.execute(
            "SELECT name, config_path, env, drop_root, active_config_version, state "
            "FROM orgs WHERE name = %s",
            (name,),
        )
        return cursor.fetchone()


def org_state(connection: psycopg.Connection, name: str) -> str | None:
    """The org's lifecycle state, or ``None`` when it does not exist."""
    with connection.cursor() as cursor:
        cursor.execute("SELECT state FROM orgs WHERE name = %s", (name,))
        row = cursor.fetchone()
    return None if row is None else str(row[0])


def set_org_state(
    connection: psycopg.Connection,
    org: str,
    state: str,
    *,
    expected: str | None = None,
) -> bool:
    """Transition the org's state; with ``expected``, only from that state.

    Returns whether a row changed — a re-run of provision against an already
    active org is then a visible no-op rather than a silent overwrite.
    """
    with connection.cursor() as cursor:
        if expected is None:
            cursor.execute(
                "UPDATE orgs SET state = %s WHERE name = %s",
                (state, org),
            )
        else:
            cursor.execute(
                "UPDATE orgs SET state = %s WHERE name = %s AND state = %s",
                (state, org, expected),
            )
        changed = cursor.rowcount == 1
    connection.commit()
    return changed


def enqueue(
    connection: psycopg.Connection,
    org: str,
    kind: str,
    *,
    params: dict[str, Any] | None = None,
    idempotency_key: str | None = None,
    priority: int = 0,
    max_attempts: int = 3,
) -> Job:
    """Queue one job, or return the job an identical idempotency key created.

    Raises:
        UnknownOrgError: the org has no row — submission is not provisioning.
        UnknownKindError: ``kind`` is outside :data:`~erserver.policy.JOB_KINDS`.
        OrgNotActiveError: the org is not ``active``. Only ``provision`` itself
            is exempt — it must run while the org is still ``provisioning``.
    """
    if kind not in JOB_KINDS:
        raise UnknownKindError(f"unknown job kind: {kind!r}")
    state = org_state(connection, org)
    if state is None:
        raise UnknownOrgError(f"unknown org: {org!r}")
    if kind != "provision" and state != "active":
        raise OrgNotActiveError(
            f"org {org!r} is {state}; jobs are accepted only when active"
        )
    job_id = str(ULID())
    with connection.cursor(row_factory=dict_row) as cursor:
        cursor.execute(
            f"INSERT INTO jobs (job_id, org, kind, params, idempotency_key, priority, "
            f"max_attempts) VALUES (%s, %s, %s, %s, %s, %s, %s) "
            f"ON CONFLICT (org, idempotency_key) WHERE idempotency_key IS NOT NULL "
            f"DO NOTHING RETURNING {_JOB_COLUMNS}",
            (job_id, org, kind, Jsonb(params or {}), idempotency_key, priority, max_attempts),
        )
        row = cursor.fetchone()
        if row is None:
            # The key was already used: return the original submission's job.
            cursor.execute(
                f"SELECT {_JOB_COLUMNS} FROM jobs WHERE org = %s AND idempotency_key = %s",
                (org, idempotency_key),
            )
            row = cursor.fetchone()
    connection.commit()
    assert row is not None
    return _job(row)


def claim(connection: psycopg.Connection) -> Job | None:
    """Claim the next runnable job, or ``None`` when the queue has nothing.

    The inner select skips jobs whose org already has an active job; the
    ``jobs_one_active_per_org`` index catches the race two dispatchers can
    still lose concurrently, and losing it is a rollback, never an error.
    """
    try:
        with connection.cursor(row_factory=dict_row) as cursor:
            cursor.execute(
                f"""
                UPDATE jobs SET state = %s, started_at = now(), updated_at = now()
                WHERE job_id = (
                  SELECT j.job_id FROM jobs j
                  WHERE j.state = %s
                    AND (j.not_before IS NULL OR j.not_before <= now())
                    AND NOT EXISTS (
                      SELECT 1 FROM jobs active
                      WHERE active.org = j.org AND active.state = ANY(%s)
                    )
                  ORDER BY j.priority DESC, j.job_id
                  FOR UPDATE SKIP LOCKED
                  LIMIT 1
                )
                RETURNING {_JOB_COLUMNS}
                """,
                (DISPATCHING, QUEUED, list(ACTIVE_STATES)),
            )
            row = cursor.fetchone()
        connection.commit()
        return None if row is None else _job(row)
    except pg_errors.UniqueViolation:
        connection.rollback()
        return None


def mark_running(connection: psycopg.Connection, job_id: str, *, run_id: str) -> None:
    """Record the run the dispatcher launched for this job."""
    with connection.cursor() as cursor:
        cursor.execute(
            "UPDATE jobs SET state = %s, run_id = %s, updated_at = now() WHERE job_id = %s",
            (RUNNING, run_id, job_id),
        )
    connection.commit()


def heartbeat(connection: psycopg.Connection, job_id: str, progress: dict[str, Any]) -> None:
    """Merge a progress payload into the job's heartbeat cache."""
    with connection.cursor() as cursor:
        cursor.execute(
            "UPDATE jobs SET progress = progress || %s, updated_at = now() WHERE job_id = %s",
            (Jsonb(progress), job_id),
        )
    connection.commit()


def finish(
    connection: psycopg.Connection,
    job_id: str,
    disposition: Disposition,
    *,
    exit_code: int | None,
    error_class: str | None,
    error_detail: str | None,
) -> None:
    """Apply a :func:`erserver.policy.dispose` verdict to the job row.

    A retry keeps its ``run_id`` — that is what lets the next dispatch resume
    the same run at its first unfinished stage — and a retry-with-resume also
    records the flag in ``params`` so the runner sees it.
    """
    sets = [
        "state = %s",
        "exit_code = %s",
        "error_class = %s",
        "error_detail = %s",
        "outcome = %s",
        "updated_at = now()",
    ]
    values: list[Any] = [
        disposition.state,
        exit_code,
        error_class,
        error_detail,
        disposition.outcome,
    ]
    if disposition.state == QUEUED:
        sets.append("not_before = now() + make_interval(secs => %s)")
        values.append(disposition.delay_seconds)
        if disposition.consume_attempt:
            sets.append("attempt = attempt + 1")
        if disposition.retry_with_resume:
            sets.append("params = params || jsonb_build_object('resume_run_id', run_id)")
    else:
        sets.append("finished_at = now()")
    values.append(job_id)
    with connection.cursor() as cursor:
        cursor.execute(f"UPDATE jobs SET {', '.join(sets)} WHERE job_id = %s", values)
    connection.commit()


def get_job(connection: psycopg.Connection, org: str, job_id: str) -> Job | None:
    with connection.cursor(row_factory=dict_row) as cursor:
        cursor.execute(
            f"SELECT {_JOB_COLUMNS} FROM jobs WHERE org = %s AND job_id = %s", (org, job_id)
        )
        row = cursor.fetchone()
    return None if row is None else _job(row)


def list_jobs(
    connection: psycopg.Connection,
    org: str,
    *,
    state: str | None = None,
    limit: int = 50,
) -> list[Job]:
    with connection.cursor(row_factory=dict_row) as cursor:
        if state is None:
            cursor.execute(
                f"SELECT {_JOB_COLUMNS} FROM jobs WHERE org = %s ORDER BY job_id DESC LIMIT %s",
                (org, limit),
            )
        else:
            cursor.execute(
                f"SELECT {_JOB_COLUMNS} FROM jobs WHERE org = %s AND state = %s "
                f"ORDER BY job_id DESC LIMIT %s",
                (org, state, limit),
            )
        rows = cursor.fetchall()
    return [_job(row) for row in rows]


def cancel(connection: psycopg.Connection, org: str, job_id: str) -> str | None:
    """Request cancellation; returns the resulting state or ``None`` for a no-op.

    A queued job cancels immediately. An active job moves to ``canceling`` and
    the dispatcher terminates its runner at the next opportunity — kill and
    resume-at-stage-boundary, the honest limitation §5 states.
    """
    with connection.cursor() as cursor:
        cursor.execute(
            "UPDATE jobs SET state = %s, finished_at = now(), updated_at = now() "
            "WHERE org = %s AND job_id = %s AND state = %s",
            (CANCELED, org, job_id, QUEUED),
        )
        if cursor.rowcount == 1:
            connection.commit()
            return CANCELED
        cursor.execute(
            "UPDATE jobs SET state = %s, updated_at = now() "
            "WHERE org = %s AND job_id = %s AND state IN (%s, %s)",
            (CANCELING, org, job_id, DISPATCHING, RUNNING),
        )
        requested = cursor.rowcount == 1
    connection.commit()
    return CANCELING if requested else None


def job_state(connection: psycopg.Connection, job_id: str) -> str | None:
    """The job's current state — the dispatcher polls this while a runner lives."""
    with connection.cursor() as cursor:
        cursor.execute("SELECT state FROM jobs WHERE job_id = %s", (job_id,))
        row = cursor.fetchone()
    return None if row is None else str(row[0])


def mark_canceled(connection: psycopg.Connection, job_id: str) -> None:
    """The runner was terminated: the job is canceled but its run is resumable."""
    with connection.cursor() as cursor:
        cursor.execute(
            "UPDATE jobs SET state = %s, finished_at = now(), updated_at = now(), "
            "params = CASE WHEN run_id IS NULL THEN params "
            "ELSE params || jsonb_build_object('resume_run_id', run_id) END "
            "WHERE job_id = %s",
            (CANCELED, job_id),
        )
    connection.commit()


def resume_job(connection: psycopg.Connection, org: str, job_id: str) -> bool:
    """Requeue a failed or canceled job, resuming its recorded run if it has one."""
    with connection.cursor() as cursor:
        cursor.execute(
            "UPDATE jobs SET state = %s, not_before = now(), updated_at = now(), "
            "finished_at = NULL, "
            "params = CASE WHEN run_id IS NULL THEN params "
            "ELSE params || jsonb_build_object('resume_run_id', run_id) END "
            "WHERE org = %s AND job_id = %s AND state IN (%s, %s)",
            (QUEUED, org, job_id, FAILED, CANCELED),
        )
        resumed = cursor.rowcount == 1
    connection.commit()
    return resumed


def reap_stale(connection: psycopg.Connection) -> int:
    """Requeue every active job at dispatcher startup.

    The skeleton dispatcher launches runners synchronously, so an active row at
    startup can only be the residue of a dispatcher that died mid-job; its run
    is resumable from the in-lake ledger (S4.7 stage-level resume).
    """
    with connection.cursor() as cursor:
        cursor.execute(
            "UPDATE jobs SET state = %s, not_before = now(), updated_at = now(), "
            "params = CASE WHEN run_id IS NULL THEN params "
            "ELSE params || jsonb_build_object('resume_run_id', run_id) END "
            "WHERE state = ANY(%s)",
            (QUEUED, list(ACTIVE_STATES)),
        )
        reaped = cursor.rowcount
    connection.commit()
    return reaped


def parse_result_line(stdout: str) -> dict[str, Any] | None:
    """The runner's terminal JSON line, or ``None`` when the process left none."""
    for line in reversed(stdout.splitlines()):
        line = line.strip()
        if line.startswith("{"):
            try:
                payload = json.loads(line)
            except json.JSONDecodeError:
                continue
            if isinstance(payload, dict) and "exit_code" in payload:
                return payload
    return None
