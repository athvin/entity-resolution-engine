"""Auto-provisioning E2E: one POST creates a tenant's dedicated database and lake.

Opt-in exactly like ``test_lake_e2e.py`` — ``ERSERVER_E2E=1`` plus a
control-plane Postgres in ``ERSERVER_TEST_DSN`` and the lake substrate (the
catalog container's ``postgres`` user is a superuser, hence CREATEDB):

    docker run -d --rm --name er-e2e-catalog -p 5434:5432 \
        -e POSTGRES_PASSWORD=er -e POSTGRES_DB=ducklake postgres:16 \
        -c max_locks_per_transaction=1024
    docker run -d --rm --name er-e2e-minio -p 9000:9000 \
        -e MINIO_ROOT_USER=minioadmin -e MINIO_ROOT_PASSWORD=minioadmin \
        minio/minio server /data          # plus a `lake` bucket

What it proves: ``POST /v1/orgs`` with only a name → the provision job creates
a brand-new Postgres database named for the tenant, ``er init`` builds every
registry relation inside it under the tenant's own S3 prefix, the org flips to
``active``, the seeded config and one-time admin key work, and the whole flow
replays idempotently.
"""

from __future__ import annotations

import os
import time
import uuid
from collections.abc import Iterator
from pathlib import Path
from typing import Any

import pytest

psycopg = pytest.importorskip("psycopg")

from erserver import db, dispatcher, provision, queue, readapi  # noqa: E402
from erserver.api import create_app  # noqa: E402
from erserver.secrets import resolve_env  # noqa: E402
from erserver.settings import ServerSettings  # noqa: E402
from fastapi.testclient import TestClient  # noqa: E402

DSN = os.environ.get("ERSERVER_TEST_DSN")
E2E = os.environ.get("ERSERVER_E2E") == "1"

pytestmark = pytest.mark.skipif(
    not (E2E and DSN),
    reason="set ERSERVER_E2E=1 and ERSERVER_TEST_DSN (plus the lake substrate) to run",
)

OPERATOR_TOKEN = "op-" + uuid.uuid4().hex

MAINT_DSN = os.environ.get(
    "ER_E2E_MAINT_DSN", "postgresql://postgres:er@localhost:5434/ducklake"
)
TENANT_DSN_TEMPLATE = os.environ.get(
    "ER_E2E_TENANT_DSN_TEMPLATE", "postgresql://postgres:er@localhost:5434/{dbname}"
)
S3_ENDPOINT = os.environ.get("ER_E2E_S3_ENDPOINT", "localhost:9000")
S3_KEY = os.environ.get("ER_E2E_S3_KEY", "minioadmin")
S3_SECRET = os.environ.get("ER_E2E_S3_SECRET", "minioadmin")

DRIVE_TIMEOUT_SECONDS = 300.0


@pytest.fixture(scope="module")
def conn() -> Iterator[psycopg.Connection]:
    assert DSN is not None
    connection = db.connect(DSN)
    db.ensure_schema(connection)
    try:
        yield connection
    finally:
        connection.close()


@pytest.fixture(scope="module")
def client(tmp_path_factory: pytest.TempPathFactory) -> Iterator[TestClient]:
    assert DSN is not None
    root = tmp_path_factory.mktemp("prov-e2e")
    # The runner subprocess inherits both from the dispatcher's process env:
    # the maintenance DSN it needs for CREATE DATABASE, and the secret the
    # org env references.
    os.environ["ERSERVER_MAINT_DSN"] = MAINT_DSN
    os.environ["ERSERVER_SECRET_E2E_S3"] = S3_SECRET
    settings = ServerSettings(
        dsn=DSN,
        operator_token=OPERATOR_TOKEN,
        maint_dsn=MAINT_DSN,
        tenant_dsn_template=TENANT_DSN_TEMPLATE,
        lake_data_path_template="s3://lake/{ns}/",
        config_root=str(root / "configs"),
        drop_root=str(root / "drop"),
        tenant_env_extra={
            "ER_S3_ENDPOINT": S3_ENDPOINT,
            "ER_S3_ACCESS_KEY_ID": S3_KEY,
            "ER_S3_SECRET_ACCESS_KEY": "secret://E2E_S3",
            "ER_S3_REGION": "us-east-1",
            "ER_S3_URL_STYLE": "path",
            "ER_S3_USE_SSL": "false",
            "ER_DUCKDB_THREADS": "4",
            "ER_DUCKDB_MEMORY_LIMIT": "4GB",
            "ER_DUCKDB_EXTENSION_DIR": str(root / "ext"),
            "DBT_PROFILES_DIR": "dbt/profiles",
        },
    )
    app = create_app(settings)
    with TestClient(app) as test_client:
        yield test_client


def operator() -> dict[str, str]:
    return {"Authorization": f"Bearer {OPERATOR_TOKEN}"}


def drive_until_quiet(connection: psycopg.Connection, org: str) -> None:
    """Run the dispatcher until the org has no queued or active jobs."""
    deadline = time.monotonic() + DRIVE_TIMEOUT_SECONDS
    while time.monotonic() < deadline:
        worked = dispatcher.tick(connection)
        remaining = [
            job
            for job in queue.list_jobs(connection, org, limit=200)
            if job.state in ("queued", "dispatching", "running", "canceling")
        ]
        if not worked and not remaining:
            return
        if not worked:
            time.sleep(0.2)
    states = [(j.job_id, j.kind, j.state) for j in queue.list_jobs(connection, org, limit=200)]
    raise AssertionError(f"dispatcher did not go quiet in time: {states}")


def test_one_post_provisions_a_dedicated_database(
    conn: psycopg.Connection, client: TestClient
) -> None:
    org = f"auto-{uuid.uuid4().hex[:8]}"
    db_name = provision.tenant_db_name(org)
    tenant = provision.tenant_namespace(org)

    # ---- 1. sign the tenant on ---------------------------------------------
    created = client.post("/v1/orgs", json={"name": org}, headers=operator())
    assert created.status_code == 201, created.text
    body = created.json()
    assert body["state"] == "provisioning"
    admin = {"Authorization": f"Bearer {body['admin_key']}"}

    # Pipeline jobs are refused until the org's lake exists.
    refused = client.post(
        f"/v1/orgs/{org}/jobs",
        json={"kind": "run_all_full"},
        headers={**admin, "Idempotency-Key": "early"},
    )
    assert refused.status_code == 409

    # ---- 2. the provision job runs (real runner subprocess) ----------------
    drive_until_quiet(conn, org)
    job = queue.get_job(conn, org, body["job_id"])
    assert job is not None
    assert (job.state, job.exit_code) == ("succeeded", 0), (job.error_class, job.error_detail)
    assert [stage["stage"] for stage in job.progress["stages"]] == [
        "create_database",
        "init_lake",
    ]

    # ---- 3. the dedicated database exists, with the lake inside it ---------
    with psycopg.connect(MAINT_DSN, autocommit=True) as maint:
        with maint.cursor() as cursor:
            cursor.execute("SELECT 1 FROM pg_database WHERE datname = %s", (db_name,))
            assert cursor.fetchone() is not None, f"no database {db_name}"

    from er.lake.catalog import catalog_connect, read_data_path

    record = queue.org_record(conn, org)
    assert record is not None
    org_env = resolve_env(dict(record["env"]))
    with catalog_connect(org_env["ER_CATALOG_DSN"]) as catalog:
        assert read_data_path(catalog, tenant) == f"s3://lake/{tenant}/"

    with readapi.open_lake(org_env) as lake:
        relations = {
            row[0]
            for row in lake.execute(
                "SELECT table_name FROM information_schema.tables "
                "WHERE table_catalog = 'lake' AND table_schema = 'main'"
            ).fetchall()
        }
    # er init applies the ddl.py-owned registry; dbt-owned relations
    # (golden_records &c.) arrive with the first pipeline run.
    assert {"raw_records", "entities", "entity_membership", "runs"} <= relations

    # ---- 4. the org is active and the seeded config serves -----------------
    detail = client.get(f"/v1/orgs/{org}", headers=admin)
    assert detail.status_code == 200
    assert detail.json()["state"] == "active"
    active = client.get(f"/v1/orgs/{org}/config", headers=admin)
    assert active.status_code == 200
    assert active.json()["version"] == 1

    # ---- 5. replay: same job, no second key, nothing re-provisioned --------
    replay = client.post("/v1/orgs", json={"name": org}, headers=operator())
    assert replay.status_code == 201
    assert replay.json()["job_id"] == body["job_id"]
    assert "admin_key" not in replay.json()
    assert replay.json()["state"] == "active"

    # ---- 6. pipeline jobs now flow (queued is enough — no data yet) --------
    accepted = client.post(
        f"/v1/orgs/{org}/jobs",
        json={"kind": "run_all_full"},
        headers={**admin, "Idempotency-Key": "after"},
    )
    assert accepted.status_code == 202
    canceled = client.post(
        f"/v1/orgs/{org}/jobs/{accepted.json()['job_id']}:cancel", headers=admin
    )
    assert canceled.status_code == 200
