"""The dispatcher: claim jobs, launch runners, tick schedules, drain steward queues.

One leader process (docs/backend-design.md §4, §5). Runners launch as
subprocesses — the single-VM fallback; the k8s-Job launcher is a later
substitution behind the same ``launch`` seam — with the org's ``ER_*``
environment injected at spawn.

Progress needs no engine change: the engine already emits exactly one JSON
line per stage on stderr (S5.2), so the launcher streams stderr and each stage
record lands in ``jobs.progress`` while the run is still going. Cancellation
is honest kill-and-resume: a ``canceling`` job's runner gets SIGTERM, the job
becomes ``canceled``, and its run resumes from its first unfinished stage when
resubmitted (S4.7).

Each idle pass also: enqueues due cron schedules (including the correction
cadence tenant configs declare), drains staged steward actions for orgs whose
writer lock has gone quiet, and delivers ``job.completed`` webhooks.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import threading
import time
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import psycopg
from ulid import ULID

from er.lake.model import PROMOTED_COUNTERS
from erserver import db, events, notify, queue, schedules, steward
from erserver.policy import CANCELING, FAILED, SUCCEEDED, dispose
from erserver.secrets import UnresolvedSecretError, resolve_env
from erserver.settings import ServerSettings

__all__ = ["RunnerResult", "flush_webhooks", "launch_runner", "run_once", "serve", "tick"]

#: Webhook delivery moved to the event spine with the spine itself; this alias
#: keeps the dispatcher's public name (tests and shutdown call it here).
flush_webhooks = events.flush_deliveries

#: The stage-record fields kept in ``jobs.progress``: identity and status plus
#: every S5.2 promoted counter — `review_queue_added` is what the post-run
#: `review.created` event is summed from, and the rest cost nothing to keep.
_STAGE_RECORD_FIELDS: tuple[str, ...] = ("stage", "status", "exit_code", *PROMOTED_COUNTERS)


#: How often the wait loop samples the runner and the job's state, seconds.
WAIT_POLL_SECONDS = 0.5
#: Grace between SIGTERM and SIGKILL when canceling a runner.
TERMINATE_GRACE_SECONDS = 10.0


@dataclass(frozen=True)
class RunnerResult:
    """What one runner process did, as the dispatcher sees it."""

    returncode: int
    stdout: str
    stderr: str
    canceled: bool = False


LaunchFn = Callable[..., RunnerResult]


def launch_runner(
    payload: dict[str, Any],
    env: dict[str, str],
    *,
    on_stage: Callable[[dict[str, Any]], None] | None = None,
    should_cancel: Callable[[], bool] | None = None,
) -> RunnerResult:
    """Run ``python -m erserver.runner`` streaming its stderr stage records.

    ``on_stage`` fires from the launcher's own thread (never the readers) for
    each S5.2 JSON line as it arrives; ``should_cancel`` is polled and a true
    answer terminates the runner (SIGTERM, then SIGKILL after a grace period).
    """
    process = subprocess.Popen(
        [sys.executable, "-m", "erserver.runner", json.dumps(payload)],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        env=env,
    )
    stdout_parts: list[str] = []
    stderr_lines: list[str] = []
    fresh_records: list[dict[str, Any]] = []
    lock = threading.Lock()

    def read_stdout() -> None:
        assert process.stdout is not None
        for line in process.stdout:
            stdout_parts.append(line)

    def read_stderr() -> None:
        assert process.stderr is not None
        for line in process.stderr:
            stderr_lines.append(line.rstrip("\n"))
            stripped = line.strip()
            if stripped.startswith("{"):
                try:
                    record = json.loads(stripped)
                except json.JSONDecodeError:
                    continue
                if isinstance(record, dict) and "stage" in record:
                    with lock:
                        fresh_records.append(record)

    readers = [
        threading.Thread(target=read_stdout, daemon=True),
        threading.Thread(target=read_stderr, daemon=True),
    ]
    for reader in readers:
        reader.start()

    canceled = False
    while process.poll() is None:
        time.sleep(WAIT_POLL_SECONDS)
        if on_stage is not None:
            with lock:
                drained, fresh_records[:] = list(fresh_records), []
            for record in drained:
                on_stage(record)
        if not canceled and should_cancel is not None and should_cancel():
            canceled = True
            process.terminate()
            deadline = time.monotonic() + TERMINATE_GRACE_SECONDS
            while process.poll() is None and time.monotonic() < deadline:
                time.sleep(WAIT_POLL_SECONDS)
            if process.poll() is None:
                process.kill()
    for reader in readers:
        reader.join(timeout=5)
    if on_stage is not None:
        with lock:
            drained, fresh_records[:] = list(fresh_records), []
        for record in drained:
            on_stage(record)
    return RunnerResult(
        returncode=process.returncode if process.returncode is not None else -1,
        stdout="".join(stdout_parts),
        stderr="\n".join(stderr_lines),
        canceled=canceled,
    )


def run_once(connection: psycopg.Connection, *, launch: LaunchFn = launch_runner) -> bool:
    """Claim and fully process at most one job; ``False`` when the queue is idle."""
    job = queue.claim(connection)
    if job is None:
        return False
    org = queue.org_row(connection, job.org)
    if org is None:
        # No org row means no event stream to record under (org_events references
        # orgs) and no subscriptions to notify; the job ledger is the record.
        queue.finish(
            connection,
            job.job_id,
            dispose(2, "config", attempt=job.attempt, max_attempts=job.max_attempts),
            exit_code=2,
            error_class="config",
            error_detail=f"org {job.org!r} no longer exists",
        )
        return True
    config_path, env_references = org
    try:
        env_overrides = resolve_env(env_references)
    except UnresolvedSecretError as exc:
        # Operator misconfiguration, not a runnable job; the message names the
        # reference and never a value.
        queue.finish(
            connection,
            job.job_id,
            dispose(2, "config", attempt=job.attempt, max_attempts=job.max_attempts),
            exit_code=2,
            error_class="config",
            error_detail=str(exc),
        )
        failure = {
            "job_id": job.job_id,
            "kind": job.kind,
            "state": FAILED,
            "exit_code": 2,
            "error_class": "config",
            "run_id": job.run_id,
        }
        events.emit(connection, job.org, "job.completed", failure)
        events.emit(connection, job.org, "job.failed", failure)
        return True
    run_id = job.run_id or str(ULID())
    queue.mark_running(connection, job.job_id, run_id=run_id)
    payload = {
        "job_id": job.job_id,
        "kind": job.kind,
        "params": job.params,
        "config_path": config_path,
        "run_id": run_id,
    }

    stage_records: list[dict[str, Any]] = []

    def on_stage(record: dict[str, Any]) -> None:
        stage_records.append(
            {key: record.get(key) for key in _STAGE_RECORD_FIELDS if key in record}
        )
        queue.heartbeat(connection, job.job_id, {"stages": stage_records})

    def should_cancel() -> bool:
        return queue.job_state(connection, job.job_id) == CANCELING

    result = launch(
        payload,
        {**os.environ, **env_overrides},
        on_stage=on_stage,
        should_cancel=should_cancel,
    )

    if result.canceled:
        queue.mark_canceled(connection, job.job_id)
        events.emit(
            connection,
            job.org,
            "job.completed",
            {"job_id": job.job_id, "kind": job.kind, "state": "canceled", "run_id": run_id},
        )
        return True

    parsed = queue.parse_result_line(result.stdout)
    if result.returncode < 0:
        exit_code: int | None = None
        error_class: str | None = "infra"
        error_detail: str | None = f"runner killed by signal {-result.returncode}"
    elif parsed is not None:
        exit_code = int(parsed["exit_code"])
        error_class = parsed.get("error_class")
        error_detail = parsed.get("error_detail")
    else:
        exit_code = result.returncode
        error_class = None
        error_detail = (result.stderr.strip().splitlines() or ["runner wrote no result line"])[-1]

    if parsed is not None:
        queue.heartbeat(
            connection,
            job.job_id,
            {"mode": parsed.get("mode"), "stages": parsed.get("stages") or stage_records},
        )
    disposition = dispose(
        exit_code, error_class, attempt=job.attempt, max_attempts=job.max_attempts
    )
    queue.finish(
        connection,
        job.job_id,
        disposition,
        exit_code=exit_code,
        error_class=error_class,
        error_detail=error_detail,
    )
    if job.kind == "provision" and disposition.state == SUCCEEDED:
        # The org's lake exists: onboarding is complete and jobs may flow.
        # ``expected`` makes an operator's re-provision of an active org a no-op.
        queue.set_org_state(connection, job.org, "active", expected="provisioning")
    terminal = {
        "job_id": job.job_id,
        "kind": job.kind,
        "state": disposition.state,
        "outcome": disposition.outcome,
        "exit_code": exit_code,
        "error_class": error_class,
        "run_id": run_id,
        "schedule_id": job.schedule_id,
        "attempt": job.attempt,
    }
    # `job.completed` on every disposition, states distinguished in the payload —
    # the original wire contract. `job.failed` is additive: the alarm channel a
    # subscriber (and the default email recipient list) watches without parsing
    # states.
    events.emit(connection, job.org, "job.completed", terminal)
    if disposition.state == FAILED:
        events.emit(connection, job.org, "job.failed", terminal)
    if disposition.state == SUCCEEDED and job.kind in (
        "run_all_full",
        "run_all_incremental",
        "correct",
    ):
        opened = sum(int(record.get("review_queue_added") or 0) for record in stage_records)
        if opened > 0:
            # One aggregate event per run, never one per review: the count comes
            # from the streamed S5.2 stage records already in hand — no lake read.
            events.emit(
                connection,
                job.org,
                "review.created",
                {"run_id": run_id, "job_id": job.job_id, "count": opened},
            )
    return True


def tick_schedules(connection: psycopg.Connection) -> int:
    """Enqueue every due schedule; the fire-time idempotency key defeats races."""
    enqueued = 0
    for schedule, fire in schedules.due(connection):
        try:
            queue.enqueue(
                connection,
                schedule.org,
                schedule.kind,
                params=schedule.params,
                idempotency_key=f"sched:{schedule.schedule_id}:{fire.isoformat()}",
                created_by=f"schedule:{schedule.schedule_id}",
                schedule_id=schedule.schedule_id,
            )
            enqueued += 1
        except (queue.UnknownOrgError, queue.OrgNotActiveError):
            # A provisioning or suspended org's cron fires must not crash the
            # leader loop; the anchor still advances, so nothing backlogs.
            pass
        schedules.record_enqueued(connection, schedule.schedule_id, fire)
    # A quiet tick is all reads, and psycopg's implicit BEGIN would leave the
    # leader idle-in-transaction between polls — holding ACCESS SHARE locks
    # that queue a booting API's ensure_schema DDL behind them, forever.
    connection.rollback()
    return enqueued


def drain_staged(connection: psycopg.Connection) -> int:
    """Apply staged steward actions for every org with no active job."""
    with connection.cursor() as cursor:
        cursor.execute(
            "SELECT DISTINCT s.org FROM staged_steward_actions s "
            "WHERE s.state = 'pending' AND NOT EXISTS ("
            "  SELECT 1 FROM jobs j WHERE j.org = s.org "
            "  AND j.state IN ('dispatching', 'running', 'canceling'))"
        )
        quiet_orgs = [row[0] for row in cursor.fetchall()]
    applied = 0
    for org in quiet_orgs:
        record = queue.org_record(connection, org)
        if record is None:
            continue
        from er.config.loader import ConfigValidationError, load_config

        try:
            tenant = str(load_config(Path(record["config_path"])).tenant)
            env = resolve_env(dict(record["env"] or {}))
        except (ConfigValidationError, OSError, UnresolvedSecretError):
            continue  # stays pending; the API surfaces the same fault to users
        applied += steward.drain_org(connection, org, env, tenant)
    connection.rollback()  # same reasoning as tick_schedules: never idle in a read tx
    return applied


def tick(connection: psycopg.Connection, *, launch: LaunchFn = launch_runner) -> bool:
    """One dispatcher pass: schedules, staged drain, then at most one job."""
    tick_schedules(connection)
    drain_staged(connection)
    return run_once(connection, launch=launch)


def serve(settings: ServerSettings | None = None) -> None:
    """The dispatcher: a leader plus ``concurrency`` runner workers.

    Workers each hold their own connection and claim jobs independently — the
    queue's one-active-per-org index means concurrency only ever parallelizes
    across orgs, never within one (§5). The leader owns the periodic work:
    schedule ticks, staged-steward draining, and the startup reaper.
    """
    resolved = settings if settings is not None else ServerSettings.from_env()
    leader = db.connect(resolved.dsn)
    db.ensure_schema(leader)
    reaped = queue.reap_stale(leader)
    if reaped:
        sys.stderr.write(f"dispatcher: requeued {reaped} stale job(s) from a prior run\n")
    stop = threading.Event()

    def worker() -> None:
        connection = db.connect(resolved.dsn)
        try:
            while not stop.is_set():
                if not run_once(connection):
                    stop.wait(resolved.poll_seconds)
        finally:
            connection.close()

    workers = [
        threading.Thread(target=worker, name=f"er-runner-{index}", daemon=True)
        for index in range(max(1, resolved.concurrency))
    ]
    for thread in workers:
        thread.start()

    def leader_pass(name: str, pass_fn: Callable[[], object]) -> None:
        # Crash isolation: one failing periodic duty must not starve the others
        # or kill the loop; the rollback clears any aborted transaction.
        try:
            pass_fn()
        except Exception as exc:  # noqa: BLE001 - the leader loop must survive
            sys.stderr.write(f"dispatcher: {name} pass failed: {exc}\n")
            try:
                leader.rollback()
            except psycopg.Error:
                pass

    try:
        while True:
            leader_pass("schedules", lambda: tick_schedules(leader))
            leader_pass("staged", lambda: drain_staged(leader))
            leader_pass("outbox", lambda: notify.drain_outbox(leader, resolved))
            time.sleep(resolved.poll_seconds)
    finally:
        stop.set()
        for thread in workers:
            thread.join(timeout=5)
        flush_webhooks()
        leader.close()


if __name__ == "__main__":
    serve()
