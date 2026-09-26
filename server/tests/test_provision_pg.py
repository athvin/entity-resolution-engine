"""The org lifecycle against a real control-plane Postgres.

Gated on ``ERSERVER_TEST_DSN`` like ``test_pg_stack.py``. Covers the state
machine (provisioning → active), the enqueue chokepoint's refusals, the
dispatcher's flip on provision success, and the auto-mode ``POST /v1/orgs``
handler — everything short of the real dedicated database, which is
``test_provision_e2e.py``'s job.
"""

from __future__ import annotations

import json
import os
import uuid
from collections.abc import Iterator
from pathlib import Path
from typing import Any

import pytest

psycopg = pytest.importorskip("psycopg")

from erserver import db, dispatcher, provision, queue, schedules  # noqa: E402
from erserver.api import create_app  # noqa: E402
from erserver.dispatcher import RunnerResult  # noqa: E402
from erserver.policy import FAILED, SUCCEEDED, dispose  # noqa: E402
from erserver.settings import ServerSettings  # noqa: E402
from fastapi.testclient import TestClient  # noqa: E402

DSN = os.environ.get("ERSERVER_TEST_DSN")
OPERATOR_TOKEN = "op-" + uuid.uuid4().hex

pytestmark = pytest.mark.skipif(
    DSN is None, reason="ERSERVER_TEST_DSN not set; control-plane Postgres tests are opt-in"
)

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
def provisioning_org(conn: psycopg.Connection) -> str:
    name = f"tenant-{uuid.uuid4().hex[:8]}"
    queue.ensure_org(conn, name, config_path="/tmp/unused.yaml", env={}, state="provisioning")
    return name


@pytest.fixture()
def auto_client(conn: psycopg.Connection, tmp_path: Path) -> Iterator[TestClient]:
    """An app whose settings enable auto-mode provisioning."""
    assert DSN is not None
    app = create_app(
        ServerSettings(
            dsn=DSN,
            operator_token=OPERATOR_TOKEN,
            maint_dsn="postgresql://maint@unused-by-the-handler/postgres",
            tenant_dsn_template="postgresql://er:pw@catalog:5432/{dbname}",
            lake_data_path_template="s3://lake/{ns}/",
            config_root=str(tmp_path / "configs"),
            drop_root=str(tmp_path / "drop"),
            tenant_env_extra={"ER_S3_ENDPOINT": "localhost:9000"},
        )
    )
    with TestClient(app) as test_client:
        yield test_client


@pytest.fixture()
def bare_client(conn: psycopg.Connection) -> Iterator[TestClient]:
    """An app with no provisioning settings — auto mode must refuse, manual work."""
    assert DSN is not None
    app = create_app(ServerSettings(dsn=DSN, operator_token=OPERATOR_TOKEN))
    with TestClient(app) as test_client:
        yield test_client


def operator() -> dict[str, str]:
    return {"Authorization": f"Bearer {OPERATOR_TOKEN}"}


def _provision_result(payload: dict[str, Any], exit_code: int) -> RunnerResult:
    record = {
        "job_id": payload["job_id"],
        "run_id": payload["run_id"],
        "mode": "provision",
        "exit_code": exit_code,
        "error_class": "config" if exit_code == 2 else None,
        "error_detail": None,
        "stages": [],
    }
    return RunnerResult(exit_code, json.dumps(record) + "\n", "")


# --------------------------------------------------------------------------- #
# the state machine at the store layer
# --------------------------------------------------------------------------- #


def test_enqueue_refuses_everything_but_provision_until_active(
    conn: psycopg.Connection, provisioning_org: str
) -> None:
    with pytest.raises(queue.OrgNotActiveError):
        queue.enqueue(conn, provisioning_org, "run_all_full", idempotency_key="k1")
    job = queue.enqueue(conn, provisioning_org, "provision", idempotency_key="p1")
    assert job.kind == "provision"


def test_ensure_org_replay_never_resets_state(conn: psycopg.Connection) -> None:
    name = f"tenant-{uuid.uuid4().hex[:8]}"
    queue.ensure_org(conn, name, config_path="/tmp/a.yaml", state="provisioning")
    assert queue.set_org_state(conn, name, "active", expected="provisioning")
    queue.ensure_org(conn, name, config_path="/tmp/a.yaml", state="provisioning")
    assert queue.org_state(conn, name) == "active"
    # And the guarded transition is a visible no-op the second time.
    assert not queue.set_org_state(conn, name, "active", expected="provisioning")


def test_orgs_without_a_state_default_to_active(conn: psycopg.Connection) -> None:
    name = f"tenant-{uuid.uuid4().hex[:8]}"
    queue.ensure_org(conn, name, config_path="/tmp/a.yaml")
    assert queue.org_state(conn, name) == "active"
    queue.enqueue(conn, name, "run_all_full", idempotency_key="ok")


def test_schedules_refuse_the_provision_kind(
    conn: psycopg.Connection, provisioning_org: str
) -> None:
    with pytest.raises(ValueError, match="not schedulable"):
        schedules.create(conn, provisioning_org, kind="provision", cron="* * * * *")


# --------------------------------------------------------------------------- #
# the dispatcher's flip
# --------------------------------------------------------------------------- #


def test_successful_provision_flips_the_org_active(
    conn: psycopg.Connection, provisioning_org: str
) -> None:
    job = queue.enqueue(conn, provisioning_org, "provision", idempotency_key="p")

    def launch(payload: dict[str, Any], env: dict[str, str], **kwargs: Any) -> RunnerResult:
        return _provision_result(payload, 0)

    assert dispatcher.run_once(conn, launch=launch)
    done = queue.get_job(conn, provisioning_org, job.job_id)
    assert done is not None and done.state == SUCCEEDED
    assert queue.org_state(conn, provisioning_org) == "active"
    # Jobs now flow.
    queue.enqueue(conn, provisioning_org, "run_all_full", idempotency_key="after")


def test_failed_provision_leaves_the_org_provisioning(
    conn: psycopg.Connection, provisioning_org: str
) -> None:
    job = queue.enqueue(conn, provisioning_org, "provision", idempotency_key="p")

    def launch(payload: dict[str, Any], env: dict[str, str], **kwargs: Any) -> RunnerResult:
        return _provision_result(payload, 2)

    assert dispatcher.run_once(conn, launch=launch)
    done = queue.get_job(conn, provisioning_org, job.job_id)
    assert done is not None and done.state == FAILED
    assert queue.org_state(conn, provisioning_org) == "provisioning"
    with pytest.raises(queue.OrgNotActiveError):
        queue.enqueue(conn, provisioning_org, "run_all_full", idempotency_key="still-blocked")


def test_schedule_tick_survives_a_not_active_org(
    conn: psycopg.Connection, provisioning_org: str
) -> None:
    with conn.cursor() as cursor:
        cursor.execute(
            "INSERT INTO schedules (schedule_id, org, kind, cron, created_at) "
            "VALUES ('sch1', %s, 'run_all_full', '* * * * *', now() - interval '1 hour')",
            (provisioning_org,),
        )
    conn.commit()
    assert dispatcher.tick_schedules(conn) == 0  # refused, not crashed


# --------------------------------------------------------------------------- #
# the API surface
# --------------------------------------------------------------------------- #


def test_job_submission_against_a_provisioning_org_is_409(
    conn: psycopg.Connection, provisioning_org: str, bare_client: TestClient
) -> None:
    response = bare_client.post(
        f"/v1/orgs/{provisioning_org}/jobs",
        json={"kind": "run_all_full"},
        headers={**operator(), "Idempotency-Key": "k"},
    )
    assert response.status_code == 409
    assert "provisioning" in response.json()["detail"]


def test_provision_kind_is_operator_only(
    conn: psycopg.Connection, provisioning_org: str, bare_client: TestClient
) -> None:
    from erserver.auth import issue_key

    _, steward_key = issue_key(conn, provisioning_org, "steward")
    response = bare_client.post(
        f"/v1/orgs/{provisioning_org}/jobs",
        json={"kind": "provision"},
        headers={"Authorization": f"Bearer {steward_key}", "Idempotency-Key": "k"},
    )
    assert response.status_code == 403


def test_auto_mode_without_settings_is_503(bare_client: TestClient) -> None:
    response = bare_client.post("/v1/orgs", json={"name": "acme"}, headers=operator())
    assert response.status_code == 503
    assert "ERSERVER_" in response.json()["detail"]


def test_manual_mode_still_creates_an_active_org(
    conn: psycopg.Connection, bare_client: TestClient, tmp_path: Path
) -> None:
    config = tmp_path / "config.yaml"
    config.write_text("tenant: manual\n")
    response = bare_client.post(
        "/v1/orgs",
        json={"name": "manual-org", "config_path": str(config)},
        headers=operator(),
    )
    assert response.status_code == 201
    assert queue.org_state(conn, "manual-org") == "active"


def test_auto_mode_onboards_idempotently(
    conn: psycopg.Connection, auto_client: TestClient
) -> None:
    org = f"auto-{uuid.uuid4().hex[:8]}"

    first = auto_client.post("/v1/orgs", json={"name": org}, headers=operator())
    assert first.status_code == 201, first.text
    body = first.json()
    assert body["state"] == "provisioning"
    assert body["tenant"] == provision.tenant_namespace(org)
    assert body["config_version"] == 1
    assert body["admin_key"].startswith("erk_")

    record = queue.org_record(conn, org)
    assert record is not None
    assert record["env"]["ER_LAKE_METADATA_SCHEMA"] == body["tenant"]
    assert record["env"]["ER_CATALOG_DSN"].endswith(provision.tenant_db_name(org))
    assert record["env"]["ER_S3_ENDPOINT"] == "localhost:9000"
    assert Path(record["config_path"]).is_file()
    assert f"tenant: {body['tenant']}" in Path(record["config_path"]).read_text()
    # The seeded document's correction cadence became the system schedule.
    assert any(
        s.source == "config:correction_pass" for s in schedules.list_schedules(conn, org)
    )
    job = queue.get_job(conn, org, body["job_id"])
    assert job is not None and job.kind == "provision"
    assert job.params["db_name"] == provision.tenant_db_name(org)

    # The org must poll as provisioning until the job lands.
    detail = auto_client.get(
        f"/v1/orgs/{org}", headers={"Authorization": f"Bearer {body['admin_key']}"}
    )
    assert detail.status_code == 200
    assert detail.json()["state"] == "provisioning"

    replay = auto_client.post("/v1/orgs", json={"name": org}, headers=operator())
    assert replay.status_code == 201
    replay_body = replay.json()
    assert replay_body["job_id"] == body["job_id"]  # idempotency key held
    assert "admin_key" not in replay_body  # issued exactly once
    assert replay_body["config_version"] == 1  # seed replays as a no-op


def test_register_org_reports_exactly_one_creator(conn: psycopg.Connection) -> None:
    name = f"tenant-{uuid.uuid4().hex[:8]}"
    assert queue.register_org(conn, name, config_path="/tmp/a.yaml", state="provisioning")
    # The second registration neither wins nor touches the existing row.
    assert not queue.register_org(conn, name, config_path="/tmp/OTHER.yaml", state="active")
    record = queue.org_record(conn, name)
    assert record is not None
    assert (record["config_path"], record["state"]) == ("/tmp/a.yaml", "provisioning")


def test_replayed_post_repairs_a_missing_config_file(
    conn: psycopg.Connection, auto_client: TestClient
) -> None:
    """A crash between the publish commit and the file write must self-heal."""
    org = f"auto-{uuid.uuid4().hex[:8]}"
    first = auto_client.post("/v1/orgs", json={"name": org}, headers=operator())
    assert first.status_code == 201, first.text

    record = queue.org_record(conn, org)
    assert record is not None
    config_file = Path(record["config_path"])
    assert config_file.is_file()
    seeded_text = config_file.read_text()
    config_file.unlink()  # simulate the torn crash window

    replay = auto_client.post("/v1/orgs", json={"name": org}, headers=operator())
    assert replay.status_code == 201
    assert replay.json()["config_version"] == 1  # still one published version
    assert config_file.is_file()
    assert config_file.read_text() == seeded_text


def test_steward_actions_against_a_provisioning_org_are_409(
    conn: psycopg.Connection, provisioning_org: str, bare_client: TestClient
) -> None:
    response = bare_client.post(
        f"/v1/orgs/{provisioning_org}/assertions",
        json={"kind": "never", "a": "crm:1", "b": "crm:2"},
        headers=operator(),
    )
    assert response.status_code == 409
    assert "provisioning" in response.json()["detail"]


def test_ensure_schema_is_idempotent_with_the_state_column(conn: psycopg.Connection) -> None:
    db.ensure_schema(conn)
    db.ensure_schema(conn)
    with conn.cursor() as cursor:
        cursor.execute(
            "SELECT column_default FROM information_schema.columns "
            "WHERE table_name = 'orgs' AND column_name = 'state'"
        )
        row = cursor.fetchone()
    assert row is not None and "active" in str(row[0])
