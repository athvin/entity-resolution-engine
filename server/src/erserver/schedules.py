"""Cron schedules: tenant automation plus the config-declared correction pass.

The dispatcher leader ticks this table (docs/backend-design.md §5): a due
schedule enqueues its job with an idempotency key derived from the fire time,
so a tick that races or repeats cannot double-enqueue. The engine's
``correction_pass.cadence`` — declared in tenant config and executed by
nothing, until now — is synced in as a system row (``source =
'config:correction_pass'``) at config publish.
"""

from __future__ import annotations

from dataclasses import dataclass
from datetime import UTC, datetime
from typing import Any

import psycopg
from croniter import croniter
from psycopg.rows import dict_row
from psycopg.types.json import Jsonb
from ulid import ULID

from erserver.policy import JOB_KINDS

__all__ = [
    "Schedule",
    "create",
    "delete",
    "due",
    "list_schedules",
    "record_enqueued",
    "sync_correction_schedule",
]

_COLUMNS = "schedule_id, org, kind, params, cron, enabled, source, last_enqueued_at, created_at"


@dataclass(frozen=True)
class Schedule:
    schedule_id: str
    org: str
    kind: str
    params: dict[str, Any]
    cron: str
    enabled: bool
    source: str
    last_enqueued_at: datetime | None
    created_at: datetime


def _schedule(row: dict[str, Any]) -> Schedule:
    return Schedule(**row)


def _validate(kind: str, cron: str) -> None:
    if kind not in JOB_KINDS:
        raise ValueError(f"unknown job kind: {kind!r}")
    if not croniter.is_valid(cron):
        raise ValueError(f"invalid cron expression: {cron!r}")


def create(
    connection: psycopg.Connection,
    org: str,
    *,
    kind: str,
    cron: str,
    params: dict[str, Any] | None = None,
    source: str = "api",
) -> Schedule:
    _validate(kind, cron)
    schedule_id = str(ULID())
    with connection.cursor(row_factory=dict_row) as cursor:
        cursor.execute(
            f"INSERT INTO schedules (schedule_id, org, kind, params, cron, source) "
            f"VALUES (%s, %s, %s, %s, %s, %s) RETURNING {_COLUMNS}",
            (schedule_id, org, kind, Jsonb(params or {}), cron, source),
        )
        row = cursor.fetchone()
    connection.commit()
    assert row is not None
    return _schedule(row)


def list_schedules(connection: psycopg.Connection, org: str) -> list[Schedule]:
    with connection.cursor(row_factory=dict_row) as cursor:
        cursor.execute(
            f"SELECT {_COLUMNS} FROM schedules WHERE org = %s ORDER BY created_at", (org,)
        )
        rows = cursor.fetchall()
    return [_schedule(row) for row in rows]


def delete(connection: psycopg.Connection, org: str, schedule_id: str) -> bool:
    with connection.cursor() as cursor:
        cursor.execute(
            "DELETE FROM schedules WHERE org = %s AND schedule_id = %s", (org, schedule_id)
        )
        deleted = cursor.rowcount == 1
    connection.commit()
    return deleted


def next_fire(schedule: Schedule, now: datetime) -> datetime:
    """The first fire time strictly after the schedule's anchor.

    The anchor is the last enqueued fire (or creation time for a fresh
    schedule), so a dispatcher that was down through several fire times
    enqueues one catch-up run, not a backlog of every missed slot.
    """
    anchor = schedule.last_enqueued_at or schedule.created_at
    fire: datetime = croniter(schedule.cron, anchor).get_next(datetime)
    if fire <= now:
        # Catch up to the most recent due slot rather than replaying each one.
        latest: datetime = croniter(schedule.cron, now).get_prev(datetime)
        return max(fire, latest)
    return fire


def due(
    connection: psycopg.Connection, now: datetime | None = None
) -> list[tuple[Schedule, datetime]]:
    """Every enabled schedule whose next fire time has arrived, with that time."""
    moment = now if now is not None else datetime.now(tz=UTC)
    with connection.cursor(row_factory=dict_row) as cursor:
        cursor.execute(f"SELECT {_COLUMNS} FROM schedules WHERE enabled")
        rows = cursor.fetchall()
    ready: list[tuple[Schedule, datetime]] = []
    for row in rows:
        schedule = _schedule(row)
        fire = next_fire(schedule, moment)
        if fire <= moment:
            ready.append((schedule, fire))
    return ready


def record_enqueued(connection: psycopg.Connection, schedule_id: str, fire_time: datetime) -> None:
    with connection.cursor() as cursor:
        cursor.execute(
            "UPDATE schedules SET last_enqueued_at = %s WHERE schedule_id = %s",
            (fire_time, schedule_id),
        )
    connection.commit()


def sync_correction_schedule(connection: psycopg.Connection, org: str, cadence: str | None) -> None:
    """Make the org's system correction schedule match its published config.

    The engine validates ``correction_pass.cadence`` but executes nothing; this
    is where the control plane finally honours it. ``None`` removes the row.
    """
    with connection.cursor() as cursor:
        cursor.execute(
            "DELETE FROM schedules WHERE org = %s AND source = 'config:correction_pass'",
            (org,),
        )
        if cadence is not None:
            _validate("correct", cadence)
            cursor.execute(
                "INSERT INTO schedules (schedule_id, org, kind, params, cron, source) "
                "VALUES (%s, %s, 'correct', '{}'::jsonb, %s, 'config:correction_pass')",
                (str(ULID()), org, cadence),
            )
    connection.commit()
