"""The org event spine: one durable stream, fanned out to webhooks.

Every notable control-plane occurrence flows through :func:`emit` — an
`org_events` row first (the in-app feed and the audit of what was announced),
then webhook fan-out on a detached thread through the existing HMAC delivery
machinery in :mod:`erserver.webhooks`.

Email is deliberately NOT a sink here yet. :mod:`erserver.notify` owns the
outbox and its templates, but the recipient roster it would fan out to — who
subscribed to which event — is the notifications workstream's table, not this
one's. Until it exists, mail is queued explicitly by the caller that knows the
address (the BFF's invite and reset flows, through ``POST /v1/email``); when
the roster lands, ``emit`` gains one call and every event type reaches the
feed, the webhooks and the outbox from this single site.

The vocabulary is closed (:data:`EVENT_TYPES`): webhook subscriptions validate
against it at write time, so a typo'd subscription is a 422 today instead of a
subscription that silently matches nothing forever.
"""

from __future__ import annotations

import threading
from typing import Any

import psycopg
from psycopg.rows import dict_row
from psycopg.types.json import Jsonb

from erserver import webhooks

__all__ = [
    "EVENT_TYPES",
    "emit",
    "flush_deliveries",
    "list_events",
]

#: Everything the platform announces. `job.completed` fires on every terminal
#: disposition (its ``state`` field distinguishes them — wire-compatible with
#: the original single-event contract); `job.failed` is the additive alarm
#: subscribers can watch without parsing states.
EVENT_TYPES: tuple[str, ...] = (
    "job.completed",
    "job.failed",
    "review.created",
    "import.received",
    "config.published",
    "report.completed",
)

#: Live webhook delivery threads. Fire-and-forget in production; tests call
#: :func:`flush_deliveries` to make delivery deterministic.
_DELIVERIES: list[threading.Thread] = []


def flush_deliveries(timeout: float = 10.0) -> None:
    """Join outstanding webhook deliveries; for tests and orderly shutdown."""
    for thread in list(_DELIVERIES):
        thread.join(timeout)
    _DELIVERIES[:] = [thread for thread in _DELIVERIES if thread.is_alive()]


def _deliver_async(
    connection: psycopg.Connection, org: str, event_type: str, payload: dict[str, Any]
) -> None:
    """Deliver off the caller's path: targets read here, HTTP on a thread.

    A slow or hostile subscriber must never delay the queue — the DB read uses
    the caller's connection synchronously (cheap), and everything network-bound
    runs detached.
    """
    subscriptions = webhooks.targets(connection, org, event_type)
    if not subscriptions:
        return
    thread = threading.Thread(
        target=webhooks.post_all, args=(subscriptions, org, event_type, payload), daemon=True
    )
    thread.start()
    _DELIVERIES.append(thread)


def emit(
    connection: psycopg.Connection,
    org: str,
    event_type: str,
    payload: dict[str, Any],
    *,
    deliver_webhooks: bool = True,
) -> int:
    """Record one event and fan it out; returns the event's feed id.

    Raises:
        ValueError: ``event_type`` is not in :data:`EVENT_TYPES` — a caller bug,
            because every emission site names its type statically.
    """
    if event_type not in EVENT_TYPES:
        raise ValueError(f"{event_type!r} is not an event type; one of {list(EVENT_TYPES)}")
    with connection.cursor() as cursor:
        cursor.execute(
            "INSERT INTO org_events (org, event_type, payload) VALUES (%s, %s, %s) RETURNING id",
            (org, event_type, Jsonb(payload)),
        )
        row = cursor.fetchone()
    connection.commit()
    if deliver_webhooks:
        _deliver_async(connection, org, event_type, payload)
    return int(row[0]) if row is not None else 0


def list_events(
    connection: psycopg.Connection,
    org: str,
    *,
    after_id: int | None = None,
    types: list[str] | None = None,
    limit: int = 50,
) -> list[dict[str, Any]]:
    """The org's events ascending from ``after_id`` — the notification poller's read.

    Ascending, not newest-first: the feed consumer tracks a high-water mark and
    asks "what happened since", so the page after a quiet poll is empty rather
    than a re-send of the newest page.
    """
    clauses = ["org = %s"]
    params: list[Any] = [org]
    if after_id is not None:
        clauses.append("id > %s")
        params.append(int(after_id))
    if types:
        clauses.append("event_type = ANY(%s)")
        params.append(list(types))
    with connection.cursor(row_factory=dict_row) as cursor:
        cursor.execute(
            f"SELECT id, event_type, payload, created_at FROM org_events "
            f"WHERE {' AND '.join(clauses)} ORDER BY id ASC LIMIT %s",
            (*params, int(limit)),
        )
        rows = cursor.fetchall()
    connection.rollback()  # a pure read; never idle-in-transaction
    return [dict(row) for row in rows]
