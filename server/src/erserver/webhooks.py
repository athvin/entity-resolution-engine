"""Webhook subscriptions and best-effort delivery of control-plane events.

The connector seam's outbound half (docs/backend-design.md §6): a connector —
or any tenant system — subscribes a URL and receives ``job.completed`` events
from the dispatcher. Delivery is at-most-a-few-tries and never blocks the
queue; a payload is signed with the subscription's secret (HMAC-SHA256 over
the body, hex, in ``X-ERServer-Signature``) so receivers can authenticate us.
"""

from __future__ import annotations

import hashlib
import hmac
import json
from dataclasses import dataclass
from typing import Any

import httpx
import psycopg
from psycopg.rows import dict_row
from psycopg.types.json import Jsonb
from ulid import ULID

__all__ = ["Webhook", "create", "deliver", "delete", "list_webhooks", "post_all", "targets"]

_COLUMNS = "webhook_id, org, url, events, secret, enabled, created_at"

DELIVERY_ATTEMPTS = 3
DELIVERY_TIMEOUT_SECONDS = 5.0


@dataclass(frozen=True)
class Webhook:
    webhook_id: str
    org: str
    url: str
    events: list[str]
    enabled: bool


def create(
    connection: psycopg.Connection,
    org: str,
    *,
    url: str,
    events: list[str] | None = None,
    secret: str | None = None,
) -> Webhook:
    webhook_id = str(ULID())
    with connection.cursor() as cursor:
        cursor.execute(
            "INSERT INTO webhooks (webhook_id, org, url, events, secret) "
            "VALUES (%s, %s, %s, %s, %s)",
            (webhook_id, org, url, Jsonb(events or ["job.completed"]), secret),
        )
    connection.commit()
    return Webhook(webhook_id, org, url, events or ["job.completed"], True)


def list_webhooks(connection: psycopg.Connection, org: str) -> list[Webhook]:
    with connection.cursor(row_factory=dict_row) as cursor:
        cursor.execute(
            f"SELECT {_COLUMNS} FROM webhooks WHERE org = %s ORDER BY created_at", (org,)
        )
        rows = cursor.fetchall()
    return [
        Webhook(row["webhook_id"], row["org"], row["url"], list(row["events"]), row["enabled"])
        for row in rows
    ]


def delete(connection: psycopg.Connection, org: str, webhook_id: str) -> bool:
    with connection.cursor() as cursor:
        cursor.execute("DELETE FROM webhooks WHERE org = %s AND webhook_id = %s", (org, webhook_id))
        deleted = cursor.rowcount == 1
    connection.commit()
    return deleted


def targets(connection: psycopg.Connection, org: str, event: str) -> list[dict[str, Any]]:
    """The enabled subscriptions for ``event`` — the only part that needs the DB."""
    with connection.cursor(row_factory=dict_row) as cursor:
        cursor.execute(
            "SELECT url, secret FROM webhooks WHERE org = %s AND enabled AND events @> %s::jsonb",
            (org, json.dumps([event])),
        )
        return list(cursor.fetchall())


def post_all(
    subscriptions: list[dict[str, Any]], org: str, event: str, payload: dict[str, Any]
) -> int:
    """POST to every subscription; pure HTTP, safe to run off the job path."""
    if not subscriptions:
        return 0
    body = json.dumps({"event": event, "org": org, **payload}, separators=(",", ":"))
    delivered = 0
    for target in subscriptions:
        headers = {"Content-Type": "application/json", "X-ERServer-Event": event}
        if target["secret"]:
            signature = hmac.new(
                target["secret"].encode(), body.encode(), hashlib.sha256
            ).hexdigest()
            headers["X-ERServer-Signature"] = signature
        for _ in range(DELIVERY_ATTEMPTS):
            try:
                response = httpx.post(
                    target["url"],
                    content=body,
                    headers=headers,
                    timeout=DELIVERY_TIMEOUT_SECONDS,
                )
                if response.status_code < 500:
                    delivered += 1
                    break
            except httpx.HTTPError:
                continue
    return delivered


def deliver(connection: psycopg.Connection, org: str, event: str, payload: dict[str, Any]) -> int:
    """Synchronous convenience: fetch targets and post inline."""
    return post_all(targets(connection, org, event), org, event, payload)
