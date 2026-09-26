"""Control-plane stack tests against a real Postgres.

Gated on ``ERSERVER_TEST_DSN`` so the bare unit runner stays service-free.
Point it at any disposable Postgres::

    docker run -d --rm -p 5433:5432 -e POSTGRES_PASSWORD=er postgres:16
    ERSERVER_TEST_DSN=postgresql://postgres:er@localhost:5433/postgres \
        uv run --project server --extra test pytest server/tests

Covers what §5/§6/§9 of docs/backend-design.md promise: constraint-backed
queueing, exit-code-keyed retries, cancellation and resume, auth and roles,
schedule ticking, the config publish workflow, imports, and webhook delivery.
The lake-backed endpoints are exercised separately by the full E2E.
"""

from __future__ import annotations

import http.server
import json
import os
import shutil
import threading
import uuid
from collections.abc import Iterator
from pathlib import Path
from typing import Any

import pytest

psycopg = pytest.importorskip("psycopg")

from erserver import configsvc, db, dispatcher, queue, schedules, steward, webhooks  # noqa: E402
from erserver.api import create_app  # noqa: E402
from erserver.auth import issue_key  # noqa: E402
from erserver.dispatcher import RunnerResult  # noqa: E402
from erserver.policy import CANCELED, FAILED, QUEUED, SUCCEEDED, dispose  # noqa: E402
from erserver.settings import ServerSettings  # noqa: E402
from fastapi.testclient import TestClient  # noqa: E402

DSN = os.environ.get("ERSERVER_TEST_DSN")
OPERATOR_TOKEN = "op-" + uuid.uuid4().hex

pytestmark = pytest.mark.skipif(
    DSN is None, reason="ERSERVER_TEST_DSN not set; control-plane Postgres tests are opt-in"
)

REPO_ROOT = Path(__file__).resolve().parents[2]
TEST_CONFIG = REPO_ROOT / "configs" / "test.yaml"

_CLEAN_ORDER = (
    "staged_steward_actions",
    "webhooks",
    "config_versions",
    "schedules",
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
    """An org whose config file is a private copy the publish flow may rewrite."""
    name = f"tenant-{uuid.uuid4().hex[:8]}"
    config_copy = tmp_path / "config.yaml"
    shutil.copy(TEST_CONFIG, config_copy)
    drop_root = tmp_path / "drop"
    drop_root.mkdir()
    queue.ensure_org(
        conn,
        name,
        config_path=str(config_copy),
        env={"ER_TEST": "1"},
        drop_root=str(drop_root),
    )
    return name


@pytest.fixture()
def client(conn: psycopg.Connection) -> Iterator[TestClient]:
    assert DSN is not None
    app = create_app(ServerSettings(dsn=DSN, operator_token=OPERATOR_TOKEN))
    with TestClient(app) as test_client:
        yield test_client


def operator() -> dict[str, str]:
    return {"Authorization": f"Bearer {OPERATOR_TOKEN}"}


def key_headers(conn: psycopg.Connection, org: str, role: str) -> dict[str, str]:
    _, full_key = issue_key(conn, org, role)
    return {"Authorization": f"Bearer {full_key}"}


# --------------------------------------------------------------------------- #
# queue + policy (through the store layer)
# --------------------------------------------------------------------------- #


def test_enqueue_is_idempotent_per_key(conn: psycopg.Connection, org: str) -> None:
    first = queue.enqueue(conn, org, "run_all_incremental", idempotency_key="k1")
    replay = queue.enqueue(conn, org, "run_all_incremental", idempotency_key="k1")
    assert replay.job_id == first.job_id
    assert queue.enqueue(conn, org, "train", idempotency_key="k2").job_id != first.job_id


def test_one_active_job_per_org_by_construction(conn: psycopg.Connection, org: str) -> None:
    first = queue.enqueue(conn, org, "run_all_incremental", idempotency_key="a")
    queue.enqueue(conn, org, "run_all_incremental", idempotency_key="b")

    claimed = queue.claim(conn)
    assert claimed is not None and claimed.job_id == first.job_id
    assert queue.claim(conn) is None  # org slot is held

    queue.finish(
        conn,
        first.job_id,
        dispose(0, None, attempt=0, max_attempts=3),
        exit_code=0,
        error_class=None,
        error_detail=None,
    )
    second = queue.claim(conn)
    assert second is not None and second.job_id != first.job_id


def test_transient_retry_keeps_the_run_and_records_resume(
    conn: psycopg.Connection, org: str
) -> None:
    job = queue.enqueue(conn, org, "run_all_incremental", idempotency_key="r")
    assert queue.claim(conn) is not None
    queue.mark_running(conn, job.job_id, run_id="01JRUN")
    queue.finish(
        conn,
        job.job_id,
        dispose(1, "transient_io", attempt=0, max_attempts=3),
        exit_code=1,
        error_class="transient_io",
        error_detail="socket reset",
    )
    retried = queue.get_job(conn, org, job.job_id)
    assert retried is not None
    assert (retried.state, retried.attempt, retried.run_id) == (QUEUED, 1, "01JRUN")
    assert retried.params["resume_run_id"] == "01JRUN"


def test_reap_stale_requeues_active_jobs_with_resume(conn: psycopg.Connection, org: str) -> None:
    job = queue.enqueue(
        conn, org, "run_all_full", params={"skip_ingest": True}, idempotency_key="stale"
    )
    assert queue.claim(conn) is not None
    queue.mark_running(conn, job.job_id, run_id="01JSTALE")

    assert queue.reap_stale(conn) == 1
    reaped = queue.get_job(conn, org, job.job_id)
    assert reaped is not None
    assert reaped.state == QUEUED
    assert reaped.params["resume_run_id"] == "01JSTALE"


# --------------------------------------------------------------------------- #
# dispatcher
# --------------------------------------------------------------------------- #


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


def test_dispatcher_run_once_end_to_end_with_fake_runner(
    conn: psycopg.Connection, org: str
) -> None:
    job = queue.enqueue(
        conn, org, "run_all_incremental", params={"skip_ingest": True}, idempotency_key="e2e"
    )
    seen: dict[str, Any] = {}

    def fake_launch(payload: dict[str, Any], env: dict[str, str], **kwargs: Any) -> RunnerResult:
        seen["payload"], seen["env"] = payload, env
        return _result(payload, 0, stages=[{"stage": "standardize", "exit_code": 10}])

    assert dispatcher.run_once(conn, launch=fake_launch)
    assert seen["env"]["ER_TEST"] == "1"
    done = queue.get_job(conn, org, job.job_id)
    assert done is not None
    assert (done.state, done.outcome) == (SUCCEEDED, "completed")
    assert done.progress["stages"] == [{"stage": "standardize", "exit_code": 10}]
    assert not dispatcher.run_once(conn, launch=fake_launch)


def test_dispatcher_streams_real_runner_stage_records(conn: psycopg.Connection, org: str) -> None:
    """The real subprocess runner: its S5.2 stderr line lands in jobs.progress.

    On a runner with no lake environment the chain refuses at standardize with
    the engine's own ERR_ENV_MISSING — which is exactly the streamed record and
    terminal result this asserts.
    """
    job = queue.enqueue(
        conn, org, "run_all_incremental", params={"skip_ingest": True}, idempotency_key="real"
    )
    assert dispatcher.run_once(conn)  # real launch_runner
    done = queue.get_job(conn, org, job.job_id)
    assert done is not None
    assert done.state == FAILED
    assert done.exit_code == 2
    assert "ERR_ENV_MISSING" in (done.error_detail or "")
    stages = done.progress.get("stages", [])
    assert stages and stages[0]["stage"] == "standardize"


def test_cancel_running_job_terminates_and_leaves_a_resumable_job(
    conn: psycopg.Connection, org: str
) -> None:
    job = queue.enqueue(
        conn, org, "run_all_full", params={"skip_ingest": True}, idempotency_key="cancelme"
    )

    def cancel_mid_run(payload: dict[str, Any], env: dict[str, str], **kwargs: Any) -> RunnerResult:
        assert queue.cancel(conn, org, payload["job_id"]) == "canceling"
        assert kwargs["should_cancel"]()
        return RunnerResult(-15, "", "", canceled=True)

    assert dispatcher.run_once(conn, launch=cancel_mid_run)
    done = queue.get_job(conn, org, job.job_id)
    assert done is not None
    assert done.state == CANCELED
    assert done.params["resume_run_id"] == done.run_id

    assert queue.resume_job(conn, org, job.job_id)
    resumed = queue.get_job(conn, org, job.job_id)
    assert resumed is not None and resumed.state == QUEUED


# --------------------------------------------------------------------------- #
# auth + API
# --------------------------------------------------------------------------- #


def test_every_endpoint_requires_a_credential(client: TestClient, org: str) -> None:
    assert client.get(f"/v1/orgs/{org}/jobs").status_code == 401
    assert client.post("/v1/orgs", json={"name": "x", "config_path": "y"}).status_code == 401
    assert client.get("/healthz").status_code == 200  # the one open route


def test_operator_manages_orgs_and_keys_and_roles_bind(
    client: TestClient, conn: psycopg.Connection, org: str, tmp_path: Path
) -> None:
    created = client.post(
        "/v1/orgs",
        json={"name": "acme", "config_path": str(tmp_path / "acme.yaml")},
        headers=operator(),
    )
    assert created.status_code == 201

    issued = client.post(f"/v1/orgs/{org}/api-keys", json={"role": "steward"}, headers=operator())
    assert issued.status_code == 201
    steward_headers = {"Authorization": f"Bearer {issued.json()['key']}"}

    viewer_headers = key_headers(conn, org, "viewer")

    # Viewer reads but cannot submit.
    assert client.get(f"/v1/orgs/{org}/jobs", headers=viewer_headers).status_code == 200
    refused = client.post(
        f"/v1/orgs/{org}/jobs",
        json={"kind": "train"},
        headers={**viewer_headers, "Idempotency-Key": "v1"},
    )
    assert refused.status_code == 403

    # Steward submits.
    accepted = client.post(
        f"/v1/orgs/{org}/jobs",
        json={"kind": "train"},
        headers={**steward_headers, "Idempotency-Key": "s1"},
    )
    assert accepted.status_code == 202

    # A key never crosses orgs: the other org 404s without enumeration.
    assert client.get("/v1/orgs/acme/jobs", headers=steward_headers).status_code == 404

    # Revocation kills the key.
    key_id = issued.json()["key_id"]
    assert client.delete(f"/v1/orgs/{org}/api-keys/{key_id}", headers=operator()).json() == {
        "revoked": True
    }
    assert client.get(f"/v1/orgs/{org}/jobs", headers=steward_headers).status_code == 401


def test_job_lifecycle_via_api(client: TestClient, conn: psycopg.Connection, org: str) -> None:
    steward_headers = key_headers(conn, org, "steward")

    submitted = client.post(
        f"/v1/orgs/{org}/jobs",
        json={"kind": "run_all_incremental", "params": {"skip_ingest": True}},
        headers={**steward_headers, "Idempotency-Key": "api-1"},
    ).json()
    replay = client.post(
        f"/v1/orgs/{org}/jobs",
        json={"kind": "run_all_incremental"},
        headers={**steward_headers, "Idempotency-Key": "api-1"},
    ).json()
    assert replay["job_id"] == submitted["job_id"]

    canceled = client.post(
        f"/v1/orgs/{org}/jobs/{submitted['job_id']}:cancel", headers=steward_headers
    )
    assert canceled.json()["state"] == CANCELED

    resumed = client.post(
        f"/v1/orgs/{org}/jobs/{submitted['job_id']}:resume", headers=steward_headers
    )
    assert resumed.json()["state"] == QUEUED

    listed = client.get(
        f"/v1/orgs/{org}/jobs", params={"state": QUEUED}, headers=steward_headers
    ).json()
    assert [item["job_id"] for item in listed] == [submitted["job_id"]]


# --------------------------------------------------------------------------- #
# schedules
# --------------------------------------------------------------------------- #


def test_schedule_ticks_once_per_fire_time(conn: psycopg.Connection, org: str) -> None:
    created = schedules.create(
        conn, org, kind="run_all_incremental", cron="* * * * *", params={"skip_ingest": True}
    )
    # A fresh schedule anchors at creation and fires at the NEXT cron slot; age
    # the anchor so a slot has already passed, as it will have in production.
    with conn.cursor() as cursor:
        cursor.execute(
            "UPDATE schedules SET created_at = now() - interval '2 minutes' WHERE schedule_id = %s",
            (created.schedule_id,),
        )
    conn.commit()
    first = dispatcher.tick_schedules(conn)
    second = dispatcher.tick_schedules(conn)
    assert first == 1
    assert second == 0  # same fire slot: anchored, not replayed
    jobs = queue.list_jobs(conn, org)
    assert len(jobs) == 1
    assert jobs[0].idempotency_key is not None and jobs[0].idempotency_key.startswith("sched:")


def test_schedule_api_validates_cron_and_kind(
    client: TestClient, conn: psycopg.Connection, org: str
) -> None:
    admin_headers = key_headers(conn, org, "admin")
    bad = client.post(
        f"/v1/orgs/{org}/schedules",
        json={"kind": "run_all_incremental", "cron": "not a cron"},
        headers=admin_headers,
    )
    assert bad.status_code == 422
    good = client.post(
        f"/v1/orgs/{org}/schedules",
        json={"kind": "correct", "cron": "0 3 * * 0"},
        headers=admin_headers,
    )
    assert good.status_code == 201
    listed = client.get(f"/v1/orgs/{org}/schedules", headers=admin_headers).json()
    assert listed[0]["cron"] == "0 3 * * 0"


# --------------------------------------------------------------------------- #
# config service
# --------------------------------------------------------------------------- #


def test_config_draft_validation_uses_the_engine_loader(
    client: TestClient, conn: psycopg.Connection, org: str
) -> None:
    admin_headers = key_headers(conn, org, "admin")
    broken = TEST_CONFIG.read_text().replace("u_seed: 20260101", "")
    refused = client.post(
        f"/v1/orgs/{org}/config/versions", json={"yaml": broken}, headers=admin_headers
    )
    assert refused.status_code == 422
    assert "/training/u_seed" in refused.json()["detail"]

    accepted = client.post(
        f"/v1/orgs/{org}/config/versions",
        json={"yaml": TEST_CONFIG.read_text()},
        headers=admin_headers,
    )
    assert accepted.status_code == 201
    assert accepted.json()["version"] == 1


def test_publish_flow_first_then_tier_a_then_operator_only(
    client: TestClient, conn: psycopg.Connection, org: str
) -> None:
    admin_headers = key_headers(conn, org, "admin")
    base = TEST_CONFIG.read_text()

    v1 = client.post(
        f"/v1/orgs/{org}/config/versions", json={"yaml": base}, headers=admin_headers
    ).json()["version"]
    published = client.post(
        f"/v1/orgs/{org}/config/versions/{v1}:publish", headers=admin_headers
    ).json()
    # First publish is tier C: train + full rebuild enqueued, cadence synced.
    assert published["tier"] == "C"
    assert len(published["jobs_enqueued"]) == 2
    kinds = [queue.get_job(conn, org, job_id).kind for job_id in published["jobs_enqueued"]]  # type: ignore[union-attr]
    assert kinds == ["train", "run_all_full"]
    rows = schedules.list_schedules(conn, org)
    assert any(s.source == "config:correction_pass" and s.cron == "0 3 * * 0" for s in rows)
    record = queue.org_record(conn, org)
    assert record is not None
    assert Path(record["config_path"]).read_text() == base

    # Threshold tweak → tier A → one rebuild job, no train.
    tweaked = base.replace("review_low: 0.60", "review_low: 0.55")
    v2 = client.post(
        f"/v1/orgs/{org}/config/versions", json={"yaml": tweaked}, headers=admin_headers
    ).json()["version"]
    second = client.post(
        f"/v1/orgs/{org}/config/versions/{v2}:publish", headers=admin_headers
    ).json()
    assert second["tier"] == "A"
    assert "thresholds" in second["changed_blocks"]
    assert len(second["jobs_enqueued"]) == 1

    # Operator-only block (training) refuses tenant admin, allows operator.
    retrained = tweaked.replace("u_max_pairs: 1000000", "u_max_pairs: 500000")
    v3 = client.post(
        f"/v1/orgs/{org}/config/versions", json={"yaml": retrained}, headers=admin_headers
    ).json()["version"]
    refused = client.post(f"/v1/orgs/{org}/config/versions/{v3}:publish", headers=admin_headers)
    assert refused.status_code == 403
    allowed = client.post(f"/v1/orgs/{org}/config/versions/{v3}:publish", headers=operator())
    assert allowed.status_code == 200
    assert allowed.json()["tier"] == "C"

    active = client.get(f"/v1/orgs/{org}/config", headers=admin_headers).json()
    assert active["version"] == v3


def test_classify_tier_matrix() -> None:
    base, _ = configsvc.validate_yaml(TEST_CONFIG.read_text())
    same, _ = configsvc.validate_yaml(TEST_CONFIG.read_text())
    tier, changed, operator_only = configsvc.classify_tier(base, same)
    assert (tier, changed, operator_only) == (None, [], [])

    blocking_changed, _ = configsvc.validate_yaml(
        TEST_CONFIG.read_text().replace("email_exact", "email_exact2")
    )
    tier, changed, _ = configsvc.classify_tier(base, blocking_changed)
    assert tier == "B" and "blocking" in changed


# --------------------------------------------------------------------------- #
# imports + webhooks
# --------------------------------------------------------------------------- #


def test_import_upload_lands_in_drop_dir_and_enqueues(
    client: TestClient, conn: psycopg.Connection, org: str
) -> None:
    steward_headers = key_headers(conn, org, "steward")
    response = client.post(
        f"/v1/orgs/{org}/imports",
        params={"source": "crm"},
        files={"file": ("leads.csv", b"id,email\n1,a@b.co\n", "text/csv")},
        headers=steward_headers,
    )
    assert response.status_code == 202
    body = response.json()
    dropped = Path(body["file"])
    assert dropped.exists() and dropped.parent.name == "crm"
    job = queue.get_job(conn, org, body["job"]["job_id"])
    assert job is not None
    assert job.kind == "run_all_incremental"
    assert job.params["source"] == "crm"


class _Receiver(http.server.BaseHTTPRequestHandler):
    received: list[dict[str, Any]] = []

    def do_POST(self) -> None:  # noqa: N802 - stdlib naming
        length = int(self.headers["Content-Length"])
        body = self.rfile.read(length)
        type(self).received.append(
            {"body": json.loads(body), "signature": self.headers.get("X-ERServer-Signature")}
        )
        self.send_response(200)
        self.end_headers()

    def log_message(self, *args: Any) -> None:  # silence
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


def test_webhook_delivery_on_job_completion(
    conn: psycopg.Connection, org: str, receiver: tuple[str, list[dict[str, Any]]]
) -> None:
    url, received = receiver
    webhooks.create(conn, org, url=url, secret="shh")
    job = queue.enqueue(
        conn, org, "run_all_incremental", params={"skip_ingest": True}, idempotency_key="wh"
    )

    def fake_launch(payload: dict[str, Any], env: dict[str, str], **kwargs: Any) -> RunnerResult:
        return _result(payload, 10)

    assert dispatcher.run_once(conn, launch=fake_launch)
    dispatcher.flush_webhooks()
    assert len(received) == 1
    event = received[0]
    assert event["body"]["event"] == "job.completed"
    assert event["body"]["job_id"] == job.job_id
    assert event["body"]["outcome"] == "no_op"
    assert event["signature"] is not None


# --------------------------------------------------------------------------- #
# staged steward actions
# --------------------------------------------------------------------------- #


def _action(a: str) -> dict[str, Any]:
    return {"type": "add_assertion", "kind": "never", "a": a, "b": a + "2"}


def test_lock_conflict_stages_and_drain_applies_in_order(
    conn: psycopg.Connection, org: str, monkeypatch: pytest.MonkeyPatch
) -> None:
    from er.errors import PreconditionFailure

    def blocked(
        env: dict[str, str], tenant: str, action: dict[str, Any], actor: str
    ) -> dict[str, Any]:
        raise PreconditionFailure("writer lock held for tenant test")

    monkeypatch.setattr(steward, "apply_action", blocked)
    status, result = steward.try_apply_or_stage(conn, org, {}, "test", _action("a"), "s1")
    assert status == "staged" and result["action_id"]

    applied_order: list[str] = []

    def works(
        env: dict[str, str], tenant: str, action: dict[str, Any], actor: str
    ) -> dict[str, Any]:
        applied_order.append(action["a"])
        return {"applied": action["type"]}

    monkeypatch.setattr(steward, "apply_action", works)
    # The lock may be free now, but an action behind pending ones must queue
    # behind them: a steward's clicks land in order.
    status2, _ = steward.try_apply_or_stage(conn, org, {}, "test", _action("c"), "s1")
    assert status2 == "staged"

    assert steward.drain_org(conn, org, {}, "test") == 2
    assert applied_order == ["a", "c"]
    assert steward.pending_count(conn, org) == 0


def test_drain_marks_a_bad_action_failed_and_continues(
    conn: psycopg.Connection, org: str, monkeypatch: pytest.MonkeyPatch
) -> None:
    steward.stage_action(conn, org, {"type": "sideways"}, "s1")
    steward.stage_action(conn, org, _action("ok"), "s1")

    def works(
        env: dict[str, str], tenant: str, action: dict[str, Any], actor: str
    ) -> dict[str, Any]:
        if action.get("type") == "sideways":
            raise ValueError("unknown steward action type")
        return {"applied": action["type"]}

    monkeypatch.setattr(steward, "apply_action", works)
    assert steward.drain_org(conn, org, {}, "test") == 1
    with conn.cursor() as cursor:
        cursor.execute(
            "SELECT state FROM staged_steward_actions WHERE org = %s ORDER BY action_id", (org,)
        )
        states = [row[0] for row in cursor.fetchall()]
    assert states == ["failed", "applied"]


def test_drain_stops_when_the_lock_is_retaken(
    conn: psycopg.Connection, org: str, monkeypatch: pytest.MonkeyPatch
) -> None:
    from er.errors import PreconditionFailure

    steward.stage_action(conn, org, _action("x"), "s1")

    def blocked(
        env: dict[str, str], tenant: str, action: dict[str, Any], actor: str
    ) -> dict[str, Any]:
        raise PreconditionFailure("writer lock held")

    monkeypatch.setattr(steward, "apply_action", blocked)
    assert steward.drain_org(conn, org, {}, "test") == 0
    assert steward.pending_count(conn, org) == 1


def test_dispatcher_drain_resolves_tenant_and_secret_env(
    conn: psycopg.Connection, org: str, monkeypatch: pytest.MonkeyPatch
) -> None:
    with conn.cursor() as cursor:
        cursor.execute(
            "UPDATE orgs SET env = %s::jsonb WHERE name = %s",
            ('{"ER_S3_SECRET_ACCESS_KEY": "secret://DRAIN"}', org),
        )
    conn.commit()
    monkeypatch.setenv("ERSERVER_SECRET_DRAIN", "resolved-secret")
    steward.stage_action(conn, org, _action("z"), "s1")

    seen: dict[str, Any] = {}

    def works(
        env: dict[str, str], tenant: str, action: dict[str, Any], actor: str
    ) -> dict[str, Any]:
        seen["env"], seen["tenant"] = env, tenant
        return {"applied": action["type"]}

    monkeypatch.setattr(steward, "apply_action", works)
    assert dispatcher.drain_staged(conn) == 1
    assert seen["tenant"] == "test"  # loaded from the org's config document
    assert seen["env"]["ER_S3_SECRET_ACCESS_KEY"] == "resolved-secret"


# --------------------------------------------------------------------------- #
# secrets at dispatch, upload limits, since validation
# --------------------------------------------------------------------------- #


def test_unresolved_secret_fails_the_job_without_leaking_values(
    conn: psycopg.Connection, org: str, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.delenv("ERSERVER_SECRET_GONE", raising=False)
    with conn.cursor() as cursor:
        cursor.execute(
            "UPDATE orgs SET env = %s::jsonb WHERE name = %s",
            ('{"ER_S3_SECRET_ACCESS_KEY": "secret://GONE"}', org),
        )
    conn.commit()
    job = queue.enqueue(conn, org, "train", idempotency_key="sec")
    assert dispatcher.run_once(conn)
    failed = queue.get_job(conn, org, job.job_id)
    assert failed is not None
    assert (failed.state, failed.exit_code, failed.error_class) == (FAILED, 2, "config")
    assert "secret://GONE" in (failed.error_detail or "")


def test_import_over_the_size_cap_is_413_and_leaves_no_file(
    client: TestClient, conn: psycopg.Connection, org: str, monkeypatch: pytest.MonkeyPatch
) -> None:
    from erserver import api as api_module

    monkeypatch.setattr(api_module, "MAX_IMPORT_BYTES", 10)
    steward_headers = key_headers(conn, org, "steward")
    refused = client.post(
        f"/v1/orgs/{org}/imports",
        params={"source": "crm"},
        files={"file": ("big.csv", b"x" * 64, "text/csv")},
        headers=steward_headers,
    )
    assert refused.status_code == 413
    record = queue.org_record(conn, org)
    assert record is not None
    assert list((Path(record["drop_root"]) / "crm").glob("*")) == []


def test_merge_plans_rejects_a_malformed_since(
    client: TestClient, conn: psycopg.Connection, org: str
) -> None:
    viewer_headers = key_headers(conn, org, "viewer")
    refused = client.get(
        f"/v1/orgs/{org}/merge-plans",
        params={"since": "not-a-date"},
        headers=viewer_headers,
    )
    assert refused.status_code == 422


# --------------------------------------------------------------------------- #
# dispatcher concurrency
# --------------------------------------------------------------------------- #


def test_different_orgs_run_concurrently_same_org_never_does(
    conn: psycopg.Connection, tmp_path: Path
) -> None:
    assert DSN is not None
    orgs = []
    for index in range(2):
        name = f"conc-{uuid.uuid4().hex[:8]}"
        config_copy = tmp_path / f"c{index}.yaml"
        shutil.copy(TEST_CONFIG, config_copy)
        queue.ensure_org(conn, name, config_path=str(config_copy), env={})
        queue.enqueue(conn, name, "train", idempotency_key="conc")
        orgs.append(name)

    barrier = threading.Barrier(2, timeout=10)

    def slow(payload: dict[str, Any], env: dict[str, str], **kwargs: Any) -> RunnerResult:
        barrier.wait()  # passes only if both orgs' jobs run at the same time
        return _result(payload, 0)

    outcomes: list[bool] = []

    def work() -> None:
        worker_conn = db.connect(DSN)
        try:
            outcomes.append(dispatcher.run_once(worker_conn, launch=slow))
        finally:
            worker_conn.close()

    threads = [threading.Thread(target=work) for _ in range(2)]
    for thread in threads:
        thread.start()
    for thread in threads:
        thread.join(timeout=15)
    assert outcomes == [True, True]
    for name in orgs:
        jobs = queue.list_jobs(conn, name)
        assert jobs and jobs[0].state == SUCCEEDED
