"""The email outbox: durable queueing, templated rendering, retried draining.

Nothing sends email inline. A caller — the event spine, the BFF's `/v1/email`
relay, a report job — INSERTs an ``email_outbox`` row and returns; the
dispatcher's leader tick drains due rows through :mod:`erserver.mailer` with
capped exponential backoff. A relay outage therefore delays mail rather than
losing it, and an unconfigured relay parks every row as ``queued`` until ops
sets ``ERSERVER_SMTP_URL``.

Templates are plain Python string templates in :data:`TEMPLATES` — a template
engine would be a dependency carrying styling this product sends nobody.
Bodies carry links (``{base_url}…``), never golden-record data: email is a new
PII flow and the address is the only personal fact it may hold.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any

import psycopg
from psycopg.rows import dict_row
from psycopg.types.json import Jsonb

from erserver import mailer
from erserver.settings import ServerSettings

__all__ = [
    "MAX_SEND_ATTEMPTS",
    "TEMPLATES",
    "EmailTemplate",
    "drain_outbox",
    "queue_email",
    "render",
]

#: After this many refused attempts a row is ``failed`` and stays queryable.
MAX_SEND_ATTEMPTS = 5

#: Backoff between attempts, seconds: 15, 30, 60, 120, capped.
_BASE_DELAY_SECONDS = 15.0
_MAX_DELAY_SECONDS = 300.0


@dataclass(frozen=True)
class EmailTemplate:
    """One message shape: a subject and a body, each a ``str.format`` template."""

    subject: str
    body: str
    #: The params the templates reference; a queue call missing one is a bug
    #: caught at queue time, not a drain that fails forever.
    required: tuple[str, ...]


TEMPLATES: dict[str, EmailTemplate] = {
    # The catch-all the operator relay uses: the caller writes both halves.
    "generic": EmailTemplate(subject="{subject}", body="{body}", required=("subject", "body")),
    "invite": EmailTemplate(
        subject="You've been invited to {org} on DupeZone",
        body=(
            "{invited_by} invited you to join {org} as {role}.\n\n"
            "Accept the invitation:\n{link}\n\n"
            "The link expires in 7 days. If you weren't expecting this, ignore it."
        ),
        required=("org", "role", "invited_by", "link"),
    ),
    "password_reset": EmailTemplate(
        subject="Reset your DupeZone password",
        body=(
            "Someone asked to reset the password for this address.\n\n"
            "Reset it:\n{link}\n\n"
            "The link expires in one hour and works once. If this wasn't you, "
            "ignore it — nothing has changed."
        ),
        required=("link",),
    ),
    "job_failed": EmailTemplate(
        subject="[{org}] job {kind} failed",
        body=("Job {job_id} ({kind}) failed with {error_class}.\n\nInspect it:\n{link}\n"),
        required=("org", "kind", "job_id", "error_class", "link"),
    ),
    "report_completed": EmailTemplate(
        subject="[{org}] your {report} report is ready",
        body="The scheduled {report} report finished ({row_count} rows).\n\nDownload:\n{link}\n",
        required=("org", "report", "row_count", "link"),
    ),
}


def render(template: str, params: dict[str, Any]) -> tuple[str, str]:
    """The (subject, body) of one message.

    Raises:
        KeyError: the template name is unknown — callers name them statically.
        ValueError: a required parameter is absent.
    """
    shape = TEMPLATES[template]
    missing = [name for name in shape.required if name not in params]
    if missing:
        raise ValueError(f"email template {template!r} is missing params {missing}")
    return shape.subject.format(**params), shape.body.format(**params)


def queue_email(
    connection: psycopg.Connection,
    *,
    to: str,
    template: str,
    params: dict[str, Any],
    org: str | None = None,
) -> int:
    """Durably queue one message; the leader tick sends it. Returns the row id."""
    render(template, params)  # a malformed queue call fails HERE, loudly
    with connection.cursor() as cursor:
        cursor.execute(
            "INSERT INTO email_outbox (org, to_addr, template, params, not_before) "
            "VALUES (%s, %s, %s, %s, now()) RETURNING id",
            (org, to, template, Jsonb(params)),
        )
        row = cursor.fetchone()
    connection.commit()
    return int(row[0]) if row is not None else 0


def _retry_delay(attempt: int) -> float:
    return min(_MAX_DELAY_SECONDS, _BASE_DELAY_SECONDS * (2.0**attempt))


def drain_outbox(
    connection: psycopg.Connection, settings: ServerSettings, *, batch: int = 20
) -> int:
    """Send due rows; returns how many were sent.

    ``FOR UPDATE SKIP LOCKED`` makes a second drainer harmless. An unconfigured
    relay returns immediately — rows stay ``queued`` with their ``not_before``
    untouched, so configuration is the only thing between them and delivery.
    """
    if not mailer.configured(settings):
        return 0
    with connection.cursor(row_factory=dict_row) as cursor:
        cursor.execute(
            "SELECT id, to_addr, template, params, attempts FROM email_outbox "
            "WHERE state = 'queued' AND (not_before IS NULL OR not_before <= now()) "
            "ORDER BY id LIMIT %s FOR UPDATE SKIP LOCKED",
            (batch,),
        )
        due = cursor.fetchall()
        sent = 0
        for row in due:
            try:
                subject, body = render(str(row["template"]), dict(row["params"]))
                mailer.send(settings, to=str(row["to_addr"]), subject=subject, body=body)
            except Exception as exc:  # noqa: BLE001 - every failure is a retry row
                attempts = int(row["attempts"]) + 1
                if attempts >= MAX_SEND_ATTEMPTS:
                    cursor.execute(
                        "UPDATE email_outbox SET state = 'failed', attempts = %s, "
                        "last_error = %s WHERE id = %s",
                        (attempts, str(exc)[:500], row["id"]),
                    )
                else:
                    cursor.execute(
                        "UPDATE email_outbox SET attempts = %s, last_error = %s, "
                        "not_before = now() + make_interval(secs => %s) WHERE id = %s",
                        (attempts, str(exc)[:500], _retry_delay(int(row["attempts"])), row["id"]),
                    )
                continue
            cursor.execute(
                "UPDATE email_outbox SET state = 'sent', sent_at = now(), "
                "attempts = attempts + 1 WHERE id = %s",
                (row["id"],),
            )
            sent += 1
    connection.commit()
    return sent
