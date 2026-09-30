"""The org event spine: emit, feed reads, and the dispatcher's emission points.

Requires ``ERSERVER_TEST_DSN`` (the +Postgres tier). Webhook fan-out is asserted
through the same local HTTP receiver the webhook suite uses; the fake launcher
drives ``run_once`` exactly as `test_pg_stack` does.
"""

from __future__ import annotations

import http.server
import json
import os
import threading
import uuid
from collections.abc import Iterator
from pathlib import Path
from typing import Any

import psycopg
import pytest

pytestmark = pytest.mark.skipif(
    not os.environ.get("ERSERVER_TEST_DSN"), reason="ERSERVER_TEST_DSN not set"
)

from erserver import db, dispatcher, events, queue, webhooks  # noqa: E402
from erserver.api import create_app  # noqa: E402
from erserver.auth import issue_key  # noqa: E402
from erserver.dispatcher import RunnerResult  # noqa: E402
from erserver.settings import ServerSettings  # noqa: E402
from fastapi.testclient import TestClient  # noqa: E402

DSN = os.environ.get("ERSERVER_TEST_DSN")
OPERATOR_TOKEN = "test-operator-token"
REPO_ROOT = Path(__file__).resolve().parents[2]
TEST_CONFIG = REPO_ROOT / "configs" / "test.yaml"

_CLEAN_ORDER = (
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


@pytest.fixture()
def conn() -> Iterator[psycopg.Connection]:
    assert DSN is not None
    connection = db.connect(DSN)
    db.ensure_schema(connection)
    with connection.cursor() as cursor:
        for table in _CLEAN_ORDER:
            cursor.execute(f"DELETE FROM {table}")
    connection.commit()
    try:
        yield connection
    finally:
        connection.close()


@pytest.fixture()
def org(conn: psycopg.Connection, tmp_path: Path) -> str:
    name = f"tenant-{uuid.uuid4().hex[:8]}"
    queue.ensure_org(conn, name, config_path=str(TEST_CONFIG), env={"ER_TEST": "1"})
    return name


@pytest.fixture()
def client(conn: psycopg.Connection) -> Iterator[TestClient]:
    assert DSN is not None
    app = create_app(ServerSettings(dsn=DSN, operator_token=OPERATOR_TOKEN))
    with TestClient(app) as test_client:
        yield test_client


class _Receiver(http.server.BaseHTTPRequestHandler):
    received: list[dict[str, Any]] = []

    def do_POST(self) -> None:  # noqa: N802 - stdlib naming
        length = int(self.headers["Content-Length"])
        body = self.rfile.read(length)
        type(self).received.append({"body": json.loads(body)})
        self.send_response(200)
        self.end_headers()

    def log_message(self, *args: Any) -> None:
        return


@pytest.fixture()
def receiver() -> Iterator[tuple[str, list[dict[str, Any]]]]:
    _Receiver.received = []
    server = http.server.HTTPServer(("127.0.0.1", 0), _Receiver)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        yield f"http://127.0.0.1:{server.server_port}/hook", _Receiver.received
    finally:
        server.shutdown()


def _result(payload: dict[str, Any], exit_code: int, **extra: Any) -> RunnerResult:
    record = {
        "job_id": payload["job_id"],
        "run_id": payload["run_id"],
        "mode": "incremental",
        "exit_code": exit_code,
        "error_class": extra.get("error_class"),
        "error_detail": extra.get("error_detail"),
        "stages": extra.get("stages", []),
    }
    return RunnerResult(exit_code, json.dumps(record) + "\n", "")


def feed(connection: psycopg.Connection, org_name: str, **kwargs: Any) -> list[dict[str, Any]]:
    return events.list_events(connection, org_name, **kwargs)


def test_emit_records_validates_and_fans_out(
    conn: psycopg.Connection, org: str, receiver: tuple[str, list[dict[str, Any]]]
) -> None:
    url, received = receiver
    webhooks.create(conn, org, url=url, events=["review.created"])

    first = events.emit(conn, org, "review.created", {"count": 3})
    second = events.emit(conn, org, "import.received", {"source": "crm"})
    assert second > first
    events.flush_deliveries()

    assert [entry["body"]["event"] for entry in received] == ["review.created"]
    rows = feed(conn, org)
    assert [(row["event_type"], row["payload"]) for row in rows] == [
        ("review.created", {"count": 3}),
        ("import.received", {"source": "crm"}),
    ]
    # Keyset: everything after the first id, and a type filter.
    assert [row["id"] for row in feed(conn, org, after_id=first)] == [second]
    assert feed(conn, org, types=["review.created"], after_id=first) == []

    with pytest.raises(ValueError, match="not an event type"):
        events.emit(conn, org, "job.exploded", {})


def test_failed_job_emits_completed_and_failed(
    conn: psycopg.Connection, org: str, receiver: tuple[str, list[dict[str, Any]]]
) -> None:
    url, received = receiver
    webhooks.create(conn, org, url=url, events=["job.failed"])
    queue.enqueue(conn, org, "run_all_incremental", params={}, idempotency_key="boom")

    def fake_launch(payload: dict[str, Any], env: dict[str, str], **kwargs: Any) -> RunnerResult:
        return _result(payload, 2, error_class="config", error_detail="bad document")

    assert dispatcher.run_once(conn, launch=fake_launch)
    dispatcher.flush_webhooks()

    kinds = [row["event_type"] for row in feed(conn, org)]
    assert kinds == ["job.completed", "job.failed"]
    assert [entry["body"]["event"] for entry in received] == ["job.failed"]
    assert received[0]["body"]["error_class"] == "config"


def test_successful_run_emits_one_review_created_aggregate(
    conn: psycopg.Connection, org: str
) -> None:
    queue.enqueue(conn, org, "run_all_incremental", params={}, idempotency_key="rev")

    def fake_launch(payload: dict[str, Any], env: dict[str, str], **kwargs: Any) -> RunnerResult:
        on_stage = kwargs["on_stage"]
        on_stage({"stage": "match", "status": "succeeded", "review_queue_added": 4})
        on_stage({"stage": "reconcile", "status": "succeeded", "review_queue_added": 1})
        return _result(payload, 0)

    assert dispatcher.run_once(conn, launch=fake_launch)
    created = [row for row in feed(conn, org) if row["event_type"] == "review.created"]
    assert len(created) == 1
    assert created[0]["payload"]["count"] == 5

    # A run that opened nothing announces nothing.
    queue.enqueue(conn, org, "run_all_incremental", params={}, idempotency_key="rev2")

    def quiet(payload: dict[str, Any], env: dict[str, str], **kwargs: Any) -> RunnerResult:
        kwargs["on_stage"]({"stage": "match", "status": "succeeded", "review_queue_added": 0})
        return _result(payload, 0)

    assert dispatcher.run_once(conn, launch=quiet)
    still = [row for row in feed(conn, org) if row["event_type"] == "review.created"]
    assert len(still) == 1


def test_unresolved_secret_early_finish_emits_job_failed(
    conn: psycopg.Connection, tmp_path: Path
) -> None:
    name = f"tenant-{uuid.uuid4().hex[:8]}"
    queue.ensure_org(
        conn, name, config_path=str(TEST_CONFIG), env={"ER_KEY": "secret://ABSENT_REF"}
    )
    queue.enqueue(conn, name, "run_all_incremental", params={}, idempotency_key="sec")

    def never(payload: dict[str, Any], env: dict[str, str], **kwargs: Any) -> RunnerResult:
        raise AssertionError("the runner must not launch for an unresolvable env")

    assert dispatcher.run_once(conn, launch=never)
    kinds = [row["event_type"] for row in feed(conn, name)]
    assert kinds == ["job.completed", "job.failed"]
    failed = feed(conn, name, types=["job.failed"])[0]
    assert failed["payload"]["error_class"] == "config"


def test_events_route_and_webhook_validation(
    client: TestClient, conn: psycopg.Connection, org: str
) -> None:
    _, viewer_key = issue_key(conn, org, "viewer")
    viewer = {"Authorization": f"Bearer {viewer_key}"}
    _, admin_key = issue_key(conn, org, "admin")
    admin = {"Authorization": f"Bearer {admin_key}"}

    events.emit(conn, org, "config.published", {"version": 2})
    page = client.get(f"/v1/orgs/{org}/events", headers=viewer).json()
    assert [item["event_type"] for item in page["items"]] == ["config.published"]
    assert page["last_id"] == page["items"][0]["id"]
    quiet = client.get(
        f"/v1/orgs/{org}/events", params={"after_id": page["last_id"]}, headers=viewer
    ).json()
    assert quiet == {"items": [], "last_id": page["last_id"]}

    unknown = client.get(f"/v1/orgs/{org}/events", params={"types": "job.exploded"}, headers=viewer)
    assert unknown.status_code == 422

    refused = client.post(
        f"/v1/orgs/{org}/webhooks",
        json={"url": "https://example.test/hook", "events": ["job.exploded"]},
        headers=admin,
    )
    assert refused.status_code == 422
    accepted = client.post(
        f"/v1/orgs/{org}/webhooks",
        json={"url": "https://example.test/hook", "events": ["job.failed"]},
        headers=admin,
    )
    assert accepted.status_code == 201
