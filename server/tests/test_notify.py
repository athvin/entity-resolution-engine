"""The mailer endpoint parsing and the email outbox (queue, drain, relay).

The parsing and template halves run on the bare tier; everything touching
``email_outbox`` needs ``ERSERVER_TEST_DSN``. The SMTP wire itself is stubbed —
delivery mechanics belong to the relay, and what this suite owns is the outbox
contract: durable acceptance, backoff, the failed terminal state, and the
operator relay route.
"""

from __future__ import annotations

import os
import uuid
from collections.abc import Iterator
from typing import Any

import psycopg
import pytest
from erserver import db, mailer, notify, queue
from erserver.api import create_app
from erserver.auth import issue_key
from erserver.mailer import MailerConfigError, endpoint_from
from erserver.settings import ServerSettings
from fastapi.testclient import TestClient

DSN = os.environ.get("ERSERVER_TEST_DSN")
OPERATOR_TOKEN = "test-operator-token"

pg = pytest.mark.skipif(not DSN, reason="ERSERVER_TEST_DSN not set")

MAIL_SETTINGS = ServerSettings(
    dsn="postgresql://unused/unused",
    smtp_url="smtp://mailer:pw@relay.example:587",
    email_from="noreply@example.test",
)


# --------------------------------------------------------------------------- #
# bare tier: URL parsing and templates
# --------------------------------------------------------------------------- #


def test_endpoint_parsing_covers_both_schemes() -> None:
    plain = endpoint_from("smtp://user:p%40ss@relay.example:2525")
    assert (plain.host, plain.port, plain.implicit_tls) == ("relay.example", 2525, False)
    assert (plain.username, plain.password) == ("user", "p@ss")

    tls = endpoint_from("smtps://relay.example")
    assert (tls.port, tls.implicit_tls, tls.username) == (465, True, None)

    assert endpoint_from("smtp://relay.example").port == 587
    with pytest.raises(MailerConfigError, match="smtp"):
        endpoint_from("https://relay.example")
    with pytest.raises(MailerConfigError, match="no host"):
        endpoint_from("smtp://")


def test_configured_requires_both_halves() -> None:
    assert mailer.configured(MAIL_SETTINGS)
    assert not mailer.configured(ServerSettings(dsn="x", smtp_url="smtp://relay"))
    assert not mailer.configured(ServerSettings(dsn="x", email_from="a@b.c"))


def test_templates_render_and_validate() -> None:
    subject, body = notify.render(
        "invite",
        {"org": "acme", "role": "steward", "invited_by": "ops@acme.io", "link": "https://x/i/t"},
    )
    assert "acme" in subject and "https://x/i/t" in body

    with pytest.raises(ValueError, match="missing params"):
        notify.render("password_reset", {})
    with pytest.raises(KeyError):
        notify.render("marketing_blast", {})


# --------------------------------------------------------------------------- #
# +Postgres tier: the outbox
# --------------------------------------------------------------------------- #


@pytest.fixture()
def conn() -> Iterator[psycopg.Connection]:
    assert DSN is not None
    connection = db.connect(DSN)
    db.ensure_schema(connection)
    clean_order = (
        "email_outbox",
        "org_events",
        "webhooks",
        "staged_steward_actions",
        "schedules",
        "config_versions",
        "api_keys",
        "audit_log",
        "jobs",
        "orgs",
    )
    with connection.cursor() as cursor:
        for table in clean_order:
            cursor.execute(f"DELETE FROM {table}")
    connection.commit()
    try:
        yield connection
    finally:
        connection.close()


def outbox_row(connection: psycopg.Connection, outbox_id: int) -> dict[str, Any]:
    with connection.cursor() as cursor:
        cursor.execute(
            "SELECT state, attempts, last_error, not_before FROM email_outbox WHERE id = %s",
            (outbox_id,),
        )
        state, attempts, last_error, not_before = cursor.fetchone()  # type: ignore[misc]
    connection.rollback()
    return {
        "state": state,
        "attempts": attempts,
        "last_error": last_error,
        "not_before": not_before,
    }


@pg
def test_queue_validates_then_drain_sends(
    conn: psycopg.Connection, monkeypatch: pytest.MonkeyPatch
) -> None:
    with pytest.raises(ValueError, match="missing params"):
        notify.queue_email(conn, to="a@b.c", template="invite", params={})
    conn.rollback()

    outbox_id = notify.queue_email(
        conn, to="a@b.c", template="generic", params={"subject": "hi", "body": "there"}
    )
    delivered: list[dict[str, Any]] = []

    def fake_send(settings: ServerSettings, *, to: str, subject: str, body: str) -> None:
        delivered.append({"to": to, "subject": subject, "body": body})

    monkeypatch.setattr(mailer, "send", fake_send)
    assert notify.drain_outbox(conn, MAIL_SETTINGS) == 1
    assert delivered == [{"to": "a@b.c", "subject": "hi", "body": "there"}]
    assert outbox_row(conn, outbox_id)["state"] == "sent"
    assert notify.drain_outbox(conn, MAIL_SETTINGS) == 0, "a sent row never re-sends"


@pg
def test_unconfigured_relay_parks_rows(conn: psycopg.Connection) -> None:
    outbox_id = notify.queue_email(
        conn, to="a@b.c", template="generic", params={"subject": "s", "body": "b"}
    )
    bare = ServerSettings(dsn="postgresql://unused/unused")
    assert notify.drain_outbox(conn, bare) == 0
    row = outbox_row(conn, outbox_id)
    assert (row["state"], row["attempts"]) == ("queued", 0)


@pg
def test_refused_sends_back_off_then_fail(
    conn: psycopg.Connection, monkeypatch: pytest.MonkeyPatch
) -> None:
    outbox_id = notify.queue_email(
        conn, to="a@b.c", template="generic", params={"subject": "s", "body": "b"}
    )

    def refuse(settings: ServerSettings, **kwargs: Any) -> None:
        raise OSError("relay refused")

    monkeypatch.setattr(mailer, "send", refuse)
    assert notify.drain_outbox(conn, MAIL_SETTINGS) == 0
    row = outbox_row(conn, outbox_id)
    assert (row["state"], row["attempts"]) == ("queued", 1)
    assert "relay refused" in row["last_error"]
    assert row["not_before"] is not None

    # Not due yet: the backoff keeps it out of the next pass.
    assert notify.drain_outbox(conn, MAIL_SETTINGS) == 0
    assert outbox_row(conn, outbox_id)["attempts"] == 1

    # Force due repeatedly until the terminal state.
    for expected_attempts in range(2, notify.MAX_SEND_ATTEMPTS + 1):
        with conn.cursor() as cursor:
            cursor.execute("UPDATE email_outbox SET not_before = now() WHERE id = %s", (outbox_id,))
        conn.commit()
        notify.drain_outbox(conn, MAIL_SETTINGS)
        assert outbox_row(conn, outbox_id)["attempts"] == expected_attempts
    assert outbox_row(conn, outbox_id)["state"] == "failed"


@pg
def test_email_relay_route_is_operator_only(conn: psycopg.Connection) -> None:
    assert DSN is not None
    app = create_app(ServerSettings(dsn=DSN, operator_token=OPERATOR_TOKEN))
    org = f"tenant-{uuid.uuid4().hex[:8]}"
    queue.ensure_org(conn, org, config_path="/tmp/unused.yaml", env={})
    _, admin_key = issue_key(conn, org, "admin")

    with TestClient(app) as client:
        refused = client.post(
            "/v1/email",
            json={"to": "a@b.c", "template": "generic", "params": {"subject": "s", "body": "b"}},
            headers={"Authorization": f"Bearer {admin_key}"},
        )
        assert refused.status_code == 403

        accepted = client.post(
            "/v1/email",
            json={
                "to": "a@b.c",
                "template": "password_reset",
                "params": {"link": "https://x/reset/t"},
            },
            headers={"Authorization": f"Bearer {OPERATOR_TOKEN}"},
        )
        assert accepted.status_code == 202
        assert accepted.json()["state"] == "queued"

        malformed = client.post(
            "/v1/email",
            json={"to": "a@b.c", "template": "invite", "params": {}},
            headers={"Authorization": f"Bearer {OPERATOR_TOKEN}"},
        )
        assert malformed.status_code == 422
