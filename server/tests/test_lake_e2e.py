"""The full-stack E2E: a real lake, driven entirely through the control plane.

Opt-in with ``ERSERVER_E2E=1`` plus a control-plane Postgres in
``ERSERVER_TEST_DSN``. It also needs the lake substrate (defaults match the
disposable containers below):

    docker run -d --rm --name er-e2e-catalog -p 5434:5432 \
        -e POSTGRES_PASSWORD=er -e POSTGRES_DB=ducklake postgres:16 \
        -c max_locks_per_transaction=1024
    docker run -d --rm --name er-e2e-minio -p 9000:9000 \
        -e MINIO_ROOT_USER=minioadmin -e MINIO_ROOT_PASSWORD=minioadmin \
        minio/minio server /data          # plus a `lake` bucket

What it proves, in one continuous story (docs/backend-design.md §15 phases
1–3, with the §8/§16 mechanics): operator provisioning, API-key auth,
bootstrap through import jobs (whose first-time match refusal is the engine's
real precondition behavior), a train job, a full resolution run, incremental
runs over a mixed batch, snapshot-pinned pagination across a writer commit,
the duplicate-groups read model, a steward never-assertion applied through
apply-now and visible as an entity split, config publish (tier A) with the
correction schedule synced, merge-plan export, and webhook delivery — every
step through HTTP plus the dispatcher, never the CLI.
"""

from __future__ import annotations

import http.server
import json
import os
import threading
import time
import uuid
from collections.abc import Iterator
from pathlib import Path
from typing import Any

import pytest

psycopg = pytest.importorskip("psycopg")

import subprocess  # noqa: E402
import sys  # noqa: E402

from erserver import db, dispatcher, queue, readapi, schedules  # noqa: E402
from erserver.api import create_app  # noqa: E402
from erserver.auth import issue_key  # noqa: E402
from erserver.secrets import resolve_env  # noqa: E402
from erserver.settings import ServerSettings  # noqa: E402
from fastapi.testclient import TestClient  # noqa: E402

DSN = os.environ.get("ERSERVER_TEST_DSN")
E2E = os.environ.get("ERSERVER_E2E") == "1"

pytestmark = pytest.mark.skipif(
    not (E2E and DSN),
    reason="set ERSERVER_E2E=1 and ERSERVER_TEST_DSN (plus the lake substrate) to run",
)

REPO_ROOT = Path(__file__).resolve().parents[2]
TEST_CONFIG = REPO_ROOT / "configs" / "test.yaml"
OPERATOR_TOKEN = "op-" + uuid.uuid4().hex
SOURCES = ("crm", "billing", "webforms")

CATALOG_DSN = os.environ.get(
    "ER_E2E_CATALOG_DSN", "postgresql://postgres:er@localhost:5434/ducklake"
)
S3_ENDPOINT = os.environ.get("ER_E2E_S3_ENDPOINT", "localhost:9000")
S3_KEY = os.environ.get("ER_E2E_S3_KEY", "minioadmin")
S3_SECRET = os.environ.get("ER_E2E_S3_SECRET", "minioadmin")

DRIVE_TIMEOUT_SECONDS = 600.0


@pytest.fixture(scope="module")
def workspace(tmp_path_factory: pytest.TempPathFactory) -> dict[str, Any]:
    """Provision one tenant end to end: env, config, corpus, lake, control plane."""
    root = tmp_path_factory.mktemp("e2e")
    ns = f"e2e_{uuid.uuid4().hex[:8]}"
    drop_root = root / "drop"
    drop_root.mkdir()

    # The stored org env carries a secret REFERENCE; the value lives only in
    # the server process environment (resolved at dispatch/read time).
    os.environ["ERSERVER_SECRET_E2E_S3"] = S3_SECRET
    org_env = {
        "ER_CATALOG_DSN": CATALOG_DSN,
        "ER_S3_ENDPOINT": S3_ENDPOINT,
        "ER_S3_ACCESS_KEY_ID": S3_KEY,
        "ER_S3_SECRET_ACCESS_KEY": "secret://E2E_S3",
        "ER_S3_REGION": "us-east-1",
        "ER_S3_URL_STYLE": "path",
        "ER_S3_USE_SSL": "false",
        "ER_LAKE_DATA_PATH": f"s3://lake/{ns}/",
        "ER_LAKE_ALIAS": "lake",
        "ER_LAKE_METADATA_SCHEMA": ns,
        "ER_DUCKDB_THREADS": "4",
        "ER_DUCKDB_MEMORY_LIMIT": "4GB",
        "ER_DUCKDB_EXTENSION_DIR": str(root / "ext"),
        "DBT_PROFILES_DIR": "dbt/profiles",
    }

    config_text = TEST_CONFIG.read_text()
    config_text = config_text.replace("tenant: test", f"tenant: {ns}")
    config_text = config_text.replace('drop_dir: "/app/drop"', f'drop_dir: "{drop_root}"')
    config_text = config_text.replace('data_path: "s3://lake/er/"', f'data_path: "s3://lake/{ns}/"')
    config_text = config_text.replace(
        'model_uri_prefix: "s3://lake/models/test/"',
        f'model_uri_prefix: "s3://lake/models/{ns}/"',
    )
    config_path = root / "config.yaml"
    config_path.write_text(config_text)

    corpus = root / "corpus"
    generated = subprocess.run(
        [
            sys.executable,
            "-m",
            "fixtures.generator.cli",
            "--personas",
            "300",
            "--records",
            "800",
            "--batch",
            "150",
            "--seed",
            "42",
            "--config",
            str(config_path),
            "--incremental-scenario",
            "mixed-v1",
            "--out",
            str(corpus),
        ],
        capture_output=True,
        text=True,
        cwd=REPO_ROOT,
    )
    assert generated.returncode == 0, generated.stderr[-800:]

    # Operator provisioning: `er init` creates the tenant's lake (the design's
    # provision job; run directly here, as an operator would).
    initialized = subprocess.run(
        ["uv", "run", "er", "init", "--config", str(config_path)],
        capture_output=True,
        text=True,
        cwd=REPO_ROOT,
        env={**os.environ, **resolve_env(org_env)},
    )
    assert initialized.returncode == 0, initialized.stderr[-800:]

    return {
        "root": root,
        "ns": ns,
        "env": org_env,
        "config_path": config_path,
        "config_text": config_text,
        "corpus": corpus,
        "drop_root": drop_root,
    }


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
def client() -> Iterator[TestClient]:
    assert DSN is not None
    app = create_app(ServerSettings(dsn=DSN, operator_token=OPERATOR_TOKEN))
    with TestClient(app) as test_client:
        yield test_client


class _Receiver(http.server.BaseHTTPRequestHandler):
    received: list[dict[str, Any]] = []

    def do_POST(self) -> None:  # noqa: N802
        length = int(self.headers["Content-Length"])
        type(self).received.append(json.loads(self.rfile.read(length)))
        self.send_response(200)
        self.end_headers()

    def log_message(self, *args: Any) -> None:
        return


@pytest.fixture(scope="module")
def receiver() -> Iterator[tuple[str, list[dict[str, Any]]]]:
    _Receiver.received = []
    server = http.server.HTTPServer(("127.0.0.1", 0), _Receiver)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        yield f"http://127.0.0.1:{server.server_port}/hook", _Receiver.received
    finally:
        server.shutdown()


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


def job_state(client: TestClient, headers: dict[str, str], org: str, job_id: str) -> dict[str, Any]:
    response = client.get(f"/v1/orgs/{org}/jobs/{job_id}", headers=headers)
    assert response.status_code == 200, response.text
    return response.json()


def upload(
    client: TestClient,
    headers: dict[str, str],
    org: str,
    source: str,
    path: Path,
) -> dict[str, Any]:
    response = client.post(
        f"/v1/orgs/{org}/imports",
        params={"source": source},
        files={"file": (path.name, path.read_bytes(), "text/csv")},
        headers=headers,
    )
    assert response.status_code == 202, response.text
    return response.json()


def test_full_story_through_the_control_plane(
    workspace: dict[str, Any],
    conn: psycopg.Connection,
    client: TestClient,
    receiver: tuple[str, list[dict[str, Any]]],
) -> None:
    org = f"e2e-{workspace['ns']}"
    hook_url, hook_events = receiver

    # ---- 1. operator provisions the org and issues keys -------------------
    created = client.post(
        "/v1/orgs",
        json={
            "name": org,
            "config_path": str(workspace["config_path"]),
            "env": workspace["env"],
            "drop_root": str(workspace["drop_root"]),
        },
        headers={"Authorization": f"Bearer {OPERATOR_TOKEN}"},
    )
    assert created.status_code == 201
    _, admin_key = issue_key(conn, org, "admin")
    _, steward_key = issue_key(conn, org, "steward")
    admin = {"Authorization": f"Bearer {admin_key}"}
    steward = {"Authorization": f"Bearer {steward_key}"}

    hooked = client.post(
        f"/v1/orgs/{org}/webhooks", json={"url": hook_url, "secret": "shh"}, headers=admin
    )
    assert hooked.status_code == 201

    # ---- 2. bootstrap: initial deliveries as import jobs -------------------
    # First-time imports run the incremental chain; ingest and standardize
    # commit, and match refuses (exit 3) because no model is active yet —
    # the engine's real precondition, asserted rather than hidden.
    import_jobs = [
        upload(client, steward, org, source, workspace["corpus"] / f"{source}.csv")["job"]["job_id"]
        for source in SOURCES
    ]
    drive_until_quiet(conn, org)
    for job_id in import_jobs:
        state = job_state(client, steward, org, job_id)
        assert state["state"] == "failed"
        assert state["exit_code"] == 3
        assert state["error_class"] == "precondition"
        stages = [record["stage"] for record in state["progress"]["stages"]]
        assert stages[:2] == ["ingest", "standardize"]

    # ---- 3. train, then resolve the whole corpus ---------------------------
    trained = client.post(
        f"/v1/orgs/{org}/jobs",
        json={"kind": "train"},
        headers={**steward, "Idempotency-Key": "bootstrap-train"},
    ).json()
    full = client.post(
        f"/v1/orgs/{org}/jobs",
        json={"kind": "run_all_full", "params": {"skip_ingest": True}},
        headers={**steward, "Idempotency-Key": "bootstrap-full"},
    ).json()
    drive_until_quiet(conn, org)
    assert job_state(client, steward, org, trained["job_id"])["state"] == "succeeded"
    full_done = job_state(client, steward, org, full["job_id"])
    assert full_done["state"] == "succeeded", full_done["error_detail"]
    assert [r["stage"] for r in full_done["progress"]["stages"]] == [
        "standardize",
        "match",
        "reconcile",
        "assemble",
    ]

    # ---- 4. the read path over a real lake ---------------------------------
    metrics = client.get(f"/v1/orgs/{org}/metrics", headers=steward).json()
    assert metrics["records"] == 800
    assert 0 < metrics["entities"] < 800
    assert metrics["duplicate_groups"] > 0
    baseline_records = metrics["records"]

    page1 = client.get(
        f"/v1/orgs/{org}/golden-records", params={"limit": 50}, headers=steward
    ).json()
    assert len(page1["items"]) == 50 and page1["next_cursor"]
    pinned_snapshot = page1["snapshot"]

    total = 0
    cursor: str | None = None
    while True:
        params: dict[str, Any] = {"limit": 200}
        if cursor:
            params["cursor"] = cursor
        page = client.get(f"/v1/orgs/{org}/golden-records", params=params, headers=steward).json()
        total += len(page["items"])
        cursor = page["next_cursor"]
        if cursor is None:
            break
    assert total == metrics["entities"]

    duplicates = client.get(f"/v1/orgs/{org}/duplicates", headers=steward).json()
    assert duplicates["items"]
    group = max(duplicates["items"], key=lambda item: item["member_count"])
    detail = client.get(
        f"/v1/orgs/{org}/golden-records/{group['entity_id']}", headers=steward
    ).json()
    assert len(detail["members"]) == group["member_count"] >= 2
    assert detail["lineage"] and detail["events"]

    searched = client.get(
        f"/v1/orgs/{org}/golden-records",
        params={"q": detail["golden"]["family_name"], "limit": 10},
        headers=steward,
    ).json()
    assert searched["items"], "search over golden records returned nothing"

    runs = client.get(f"/v1/orgs/{org}/runs", headers=steward).json()
    assert any(run["mode"] == "full" and run["status"] == "succeeded" for run in runs)
    assert any(run["mode"] == "train" for run in runs)

    # ---- 4b. adopt the provisioned document as config v1 -------------------
    # The control plane had no published baseline (the file was seeded at
    # provisioning), so this first publish is tier C by definition: it trains
    # and rebuilds, and both come back green against the live corpus.
    v0 = client.post(
        f"/v1/orgs/{org}/config/versions",
        json={"yaml": workspace["config_text"]},
        headers=admin,
    )
    assert v0.status_code == 201, v0.text
    adopted = client.post(
        f"/v1/orgs/{org}/config/versions/{v0.json()['version']}:publish", headers=admin
    ).json()
    assert adopted["tier"] == "C"
    drive_until_quiet(conn, org)
    for job_id in adopted["jobs_enqueued"]:
        state = job_state(client, steward, org, job_id)
        assert state["state"] == "succeeded", (state["kind"], state["error_detail"])

    # ---- 5. incremental over the mixed batch, webhooks firing --------------
    batch_jobs = [
        upload(client, steward, org, source, workspace["corpus"] / "batch" / f"{source}.csv")[
            "job"
        ]["job_id"]
        for source in SOURCES
    ]
    drive_until_quiet(conn, org)
    for job_id in batch_jobs:
        state = job_state(client, steward, org, job_id)
        assert state["state"] == "succeeded", (state["error_class"], state["error_detail"])

    metrics_after = client.get(f"/v1/orgs/{org}/metrics", headers=steward).json()
    assert metrics_after["records"] > baseline_records

    dispatcher.flush_webhooks()
    finished_hooks = [e for e in hook_events if e.get("event") == "job.completed"]
    assert {e["job_id"] for e in finished_hooks} >= set(batch_jobs)

    # ---- 6. snapshot-pinned pagination across those commits ----------------
    replay = client.get(
        f"/v1/orgs/{org}/golden-records",
        params={"cursor": page1["next_cursor"], "limit": 50},
        headers=steward,
    ).json()
    assert replay["snapshot"] == pinned_snapshot  # a writer committed; the page did not shear

    # ---- 7. steward: never-assertion splits an entity ----------------------
    fresh_dupes = client.get(f"/v1/orgs/{org}/duplicates", headers=steward).json()["items"]
    target = max(fresh_dupes, key=lambda item: item["member_count"])
    target_detail = client.get(
        f"/v1/orgs/{org}/golden-records/{target['entity_id']}", headers=steward
    ).json()
    key_a = target_detail["members"][0]["record_key"]
    key_b = target_detail["members"][1]["record_key"]

    asserted = client.post(
        f"/v1/orgs/{org}/assertions",
        json={"kind": "never", "a": key_a, "b": key_b, "apply_now": True},
        headers=steward,
    )
    assert asserted.status_code == 201, asserted.text
    body = asserted.json()
    assert body["status"] == "applied"
    apply_job = body["apply_job"]
    drive_until_quiet(conn, org)
    assert job_state(client, steward, org, apply_job)["state"] == "succeeded"

    with readapi.open_lake(resolve_env(workspace["env"])) as lake:
        rows = lake.execute(
            "SELECT record_key, entity_id FROM lake.main.entity_membership "
            "WHERE record_key IN (?, ?)",
            [key_a, key_b],
        ).fetchall()
    assignments = dict(rows)
    assert assignments[key_a] != assignments[key_b], "never-assertion did not split the pair"

    # ---- 8. config publish: tier A widens the gray band, rebuild green ------
    v1 = client.post(
        f"/v1/orgs/{org}/config/versions",
        json={"yaml": workspace["config_text"].replace("auto_merge: 0.95", "auto_merge: 0.99")},
        headers=admin,
    )
    assert v1.status_code == 201, v1.text
    published = client.post(
        f"/v1/orgs/{org}/config/versions/{v1.json()['version']}:publish", headers=admin
    ).json()
    assert published["tier"] == "A"
    assert Path(workspace["config_path"]).read_text().count("auto_merge: 0.99") == 1
    assert any(s.source == "config:correction_pass" for s in schedules.list_schedules(conn, org))
    drive_until_quiet(conn, org)
    for job_id in published["jobs_enqueued"]:
        assert job_state(client, steward, org, job_id)["state"] == "succeeded"

    # ---- 9. reviews: the widened band must queue pairs; resolve one ---------
    reviews = client.get(f"/v1/orgs/{org}/reviews", headers=steward).json()
    assert reviews, "gray band [0.60, 0.99) produced no reviews"
    resolved = client.post(
        f"/v1/orgs/{org}/reviews/{reviews[0]['review_id']}:resolve",
        json={"resolution": "dismiss"},
        headers=steward,
    )
    assert resolved.status_code == 200, resolved.text
    assert resolved.json()["status"] in ("applied", "staged")

    # ---- 10. merge-plan export ----------------------------------------------
    plans = client.get(f"/v1/orgs/{org}/merge-plans", headers=steward).json()["items"]
    assert plans and all(plan["member_count"] >= 2 for plan in plans)
    exported = client.get(f"/v1/orgs/{org}/merge-plans", params={"format": "csv"}, headers=steward)
    assert exported.headers["content-type"].startswith("text/csv")
    lines = exported.text.strip().splitlines()
    assert lines[0].startswith("entity_id,master_key")
    assert len(lines) == len(plans) + 1
