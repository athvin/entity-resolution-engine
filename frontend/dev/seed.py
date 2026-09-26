"""Seed the frontend dev stack with one resolved tenant (org: acme-dev).

A pure-HTTP transliteration of the happy path in
server/tests/test_lake_e2e.py::test_full_story_through_the_control_plane, run
against the live dev stack (frontend/dev/up.sh must already be serving :8000):

  1. derive a tenant config from configs/test.yaml (like the e2e workspace fixture)
  2. generate the deterministic corpus (fixtures.generator.cli, seed 42)
  3. `er init` the tenant lake, register the org (manual mode), mint API keys
  4. upload the three sources — these FAIL with error_class=precondition by design
     (no model yet; the UI renders this state deliberately)
  5. run `train`, then `run_all_full` — the dispatcher executes them (~3 min)
  6. write .state/keys.json and .state/expected.json (ground truth for e2e asserts)

Run via: uv run --project server --extra test python frontend/dev/seed.py
"""

from __future__ import annotations

import csv
import json
import os
import subprocess
import sys
import time
from collections import Counter
from pathlib import Path
from typing import Any

import httpx

REPO_ROOT = Path(__file__).resolve().parents[2]
STATE = REPO_ROOT / "frontend" / "dev" / ".state"
BASE_URL = os.environ.get("ERSERVER_BASE_URL", "http://localhost:8000")
OPERATOR_TOKEN = os.environ.get("ERSERVER_OPERATOR_TOKEN", "dev-operator-token")
CATALOG_DSN = "postgresql://postgres:er@localhost:5434/ducklake"

ORG = "acme-dev"
NS = "devacme"
SOURCES = ("crm", "billing", "webforms")
JOB_TIMEOUT_SECONDS = 900.0

OPERATOR = {"Authorization": f"Bearer {OPERATOR_TOKEN}"}


def tenant_env(root: Path) -> dict[str, str]:
    return {
        "ER_CATALOG_DSN": CATALOG_DSN,
        "ER_S3_ENDPOINT": "localhost:9000",
        "ER_S3_ACCESS_KEY_ID": "minioadmin",
        "ER_S3_SECRET_ACCESS_KEY": "minioadmin",
        "ER_S3_REGION": "us-east-1",
        "ER_S3_URL_STYLE": "path",
        "ER_S3_USE_SSL": "false",
        "ER_LAKE_DATA_PATH": f"s3://lake/{NS}/",
        "ER_LAKE_ALIAS": "lake",
        "ER_LAKE_METADATA_SCHEMA": NS,
        "ER_DUCKDB_THREADS": "4",
        "ER_DUCKDB_MEMORY_LIMIT": "4GB",
        "ER_DUCKDB_EXTENSION_DIR": str(root / "ext"),
        "DBT_PROFILES_DIR": "dbt/profiles",
    }


def derive_config(drop_root: Path) -> Path:
    text = (REPO_ROOT / "configs" / "test.yaml").read_text()
    text = text.replace("tenant: test", f"tenant: {NS}")
    text = text.replace('drop_dir: "/app/drop"', f'drop_dir: "{drop_root}"')
    text = text.replace('data_path: "s3://lake/er/"', f'data_path: "s3://lake/{NS}/"')
    text = text.replace(
        'model_uri_prefix: "s3://lake/models/test/"', f'model_uri_prefix: "s3://lake/models/{NS}/"'
    )
    config_path = STATE / "seed" / "config.yaml"
    config_path.parent.mkdir(parents=True, exist_ok=True)
    config_path.write_text(text)
    return config_path


def run_checked(command: list[str], env: dict[str, str] | None = None) -> None:
    merged = {**os.environ, **(env or {})}
    result = subprocess.run(command, capture_output=True, text=True, cwd=REPO_ROOT, env=merged)
    if result.returncode != 0:
        sys.stderr.write(result.stderr[-2000:])
        raise SystemExit(f"command failed: {' '.join(command)}")


def wait_for_job(client: httpx.Client, headers: dict[str, str], job_id: str) -> dict[str, Any]:
    deadline = time.monotonic() + JOB_TIMEOUT_SECONDS
    while time.monotonic() < deadline:
        job = client.get(f"/v1/orgs/{ORG}/jobs/{job_id}", headers=headers).json()
        if job["state"] in ("succeeded", "failed", "canceled"):
            return job
        time.sleep(2.0)
    raise SystemExit(f"job {job_id} did not finish within {JOB_TIMEOUT_SECONDS}s")


def truth_summary(corpus: Path) -> dict[str, int]:
    personas = Counter[str]()
    with (corpus / "truth.csv").open() as handle:
        for row in csv.DictReader(handle):
            personas[row["persona_id"]] += 1
    return {
        "records": sum(personas.values()),
        "personas": len(personas),
        "multi_record_personas": sum(1 for count in personas.values() if count > 1),
    }


def main() -> None:
    client = httpx.Client(base_url=BASE_URL, timeout=60.0)

    if client.get("/healthz").status_code != 200:
        raise SystemExit("erserver is not responding on :8000 — run `make frontend-dev` first")
    if client.get(f"/v1/orgs/{ORG}", headers=OPERATOR).status_code == 200:
        raise SystemExit(f"org {ORG} already exists — run `make frontend-dev-reset` to start over")

    root = STATE / "seed"
    drop_root = root / "drop"
    corpus = root / "corpus"
    drop_root.mkdir(parents=True, exist_ok=True)
    (root / "ext").mkdir(parents=True, exist_ok=True)
    config_path = derive_config(drop_root)
    env = tenant_env(root)

    print("==> generating corpus (personas=300 records=800 seed=42)")
    run_checked(
        [
            "uv",
            "run",
            "python",
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
        ]
    )

    print("==> er init (tenant lake)")
    run_checked(["uv", "run", "er", "init", "--config", str(config_path)], env=env)

    print(f"==> registering org {ORG}")
    created = client.post(
        "/v1/orgs",
        json={
            "name": ORG,
            "config_path": str(config_path),
            "env": env,
            "drop_root": str(drop_root),
        },
        headers=OPERATOR,
    )
    if created.status_code != 201:
        raise SystemExit(f"org creation failed: {created.status_code} {created.text}")

    print("==> minting API keys (admin, steward, viewer)")
    keys: dict[str, dict[str, str]] = {}
    for role in ("admin", "steward", "viewer"):
        issued = client.post(f"/v1/orgs/{ORG}/api-keys", json={"role": role}, headers=OPERATOR)
        if issued.status_code != 201:
            raise SystemExit(f"key issue failed: {issued.status_code} {issued.text}")
        body = issued.json()
        keys[role] = {"key_id": body["key_id"], "key": body["key"]}
    steward = {"Authorization": f"Bearer {keys['steward']['key']}"}

    print("==> uploading initial deliveries (these fail with precondition until train — by design)")
    for source in SOURCES:
        path = corpus / f"{source}.csv"
        uploaded = client.post(
            f"/v1/orgs/{ORG}/imports",
            params={"source": source},
            files={"file": (path.name, path.read_bytes(), "text/csv")},
            headers=steward,
        )
        if uploaded.status_code != 202:
            raise SystemExit(f"import failed: {uploaded.status_code} {uploaded.text}")
        job = wait_for_job(client, steward, uploaded.json()["job"]["job_id"])
        state = f"{job['state']}/{job.get('error_class')}"
        print(f"    {source}: {state} (expected failed/precondition)")

    print("==> train (this is the slow step, ~2 min)")
    trained = client.post(
        f"/v1/orgs/{ORG}/jobs",
        json={"kind": "train"},
        headers={**steward, "Idempotency-Key": "seed-train"},
    ).json()
    job = wait_for_job(client, steward, trained["job_id"])
    if job["state"] != "succeeded":
        raise SystemExit(f"train failed: {job.get('error_class')} {job.get('error_detail')}")

    print("==> run_all_full")
    full = client.post(
        f"/v1/orgs/{ORG}/jobs",
        json={"kind": "run_all_full", "params": {"skip_ingest": True}},
        headers={**steward, "Idempotency-Key": "seed-full"},
    ).json()
    job = wait_for_job(client, steward, full["job_id"])
    if job["state"] != "succeeded":
        raise SystemExit(f"run_all_full failed: {job.get('error_class')} {job.get('error_detail')}")

    metrics = client.get(f"/v1/orgs/{ORG}/metrics", headers=steward).json()
    truth = truth_summary(corpus)
    (STATE / "keys.json").write_text(json.dumps({"org": ORG, "keys": keys}, indent=2))
    (STATE / "expected.json").write_text(
        json.dumps({"org": ORG, "metrics": metrics, "truth": truth}, indent=2)
    )

    print("==> seeded")
    print(f"    metrics:  {json.dumps(metrics)}")
    print(f"    truth:    {json.dumps(truth)}")
    print(f"    keys:     {STATE / 'keys.json'} (dev only — never commit)")
    print(f"    expected: {STATE / 'expected.json'}")


if __name__ == "__main__":
    main()
