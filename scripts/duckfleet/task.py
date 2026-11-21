#!/usr/bin/env python3
"""Run the normal ER job lifecycle against an isolated disposable DuckFleet."""

import argparse
import json
import os
from pathlib import Path
import secrets
import subprocess
import tempfile
import uuid

ROOT = Path(__file__).resolve().parents[2]


def run(source, image, output):
    project = "er-duckfleet-task-" + uuid.uuid4().hex[:12]
    output.parent.mkdir(parents=True, exist_ok=True)
    output.unlink(missing_ok=True)
    token = secrets.token_hex(32)
    password = secrets.token_hex(32)
    env = dict(
        os.environ,
        DUCKFLEET_PORT="0",
        DUCKFLEET_TOKEN=token,
        DUCKFLEET_POSTGRES_ADMIN_PASSWORD=password,
        DUCKFLEET_INTERNAL_TOKEN=secrets.token_hex(32),
    )
    with tempfile.TemporaryDirectory(prefix=project) as scratch:
        override = Path(scratch) / "compose.json"
        services = {
            name: {"image": image, "pull_policy": "never"}
            for name in ["platform-migrate", "worker-small", "server"]
        }
        services["postgres"] = {"ports": ["127.0.0.1::5432"]}
        services["object-store"] = {"ports": ["127.0.0.1::9000"]}
        override.write_text(json.dumps({"services": services}))
        command = [
            "docker",
            "compose",
            "-p",
            project,
            "-f",
            str(source / "compose.yaml"),
            "-f",
            str(override),
        ]
        with output.with_suffix(".log").open("w") as log:
            try:
                subprocess.run(
                    command + ["up", "-d", "--wait", "server"],
                    env=env,
                    stdout=log,
                    stderr=log,
                    check=True,
                )

                def address(service, port):
                    return subprocess.check_output(
                        command + ["port", service, str(port)], env=env, text=True
                    ).strip()

                pg_host, pg_port = address("postgres", 5432).rsplit(":", 1)
                proof_env = dict(
                    env,
                    ER_TEST_PG_DSN=f"host={pg_host} port={pg_port} dbname=postgres user=duckfleet password={password}",
                    DEDUPE_E2E_DUCKLAKE="1",
                    ER_DUCKFLEET_PROOF_ADDRESS=address("server", 8080),
                    ER_PROOF_TOKEN=token,
                    ER_PROOF_LAKE_CATALOG=f"postgres:host={pg_host} port={pg_port} dbname=lake user=duckfleet_catalog password=local-lake-catalog-password",
                    ER_LAKE_S3_ENDPOINT=address("object-store", 9000),
                    ER_LAKE_S3_KEY_ID="DUCKFLEETLOCAL",
                    ER_LAKE_S3_SECRET="local-object-store-password",
                    ER_LAKE_S3_USE_SSL="false",
                    ER_LAKE_S3_URL_STYLE="path",
                    ER_DUCKDB_THREADS="1",
                    ER_DUCKDB_MEMORY_LIMIT="256MB",
                )
                subprocess.run(
                    [
                        "cargo",
                        "test",
                        "-p",
                        "er-server",
                        "--features",
                        "disposable-proof",
                        "pg_duckfleet_task::",
                        "--",
                        "--nocapture",
                    ],
                    cwd=ROOT / "rust",
                    env=proof_env,
                    stdout=log,
                    stderr=log,
                    check=True,
                )
                log.flush()
                rows = [
                    json.loads(line)
                    for line in output.with_suffix(".log").read_text().splitlines()
                    if line.startswith('{"embedded_remote_equal"')
                ]
                assert len(rows) == 1, "actual remote task did not emit its evidence"
                report = rows[0]
                report["upstream_revision"] = subprocess.check_output(
                    ["git", "-C", str(source), "rev-parse", "HEAD"], text=True
                ).strip()
                container = subprocess.check_output(
                    command + ["ps", "-q", "worker-small"], env=env, text=True
                ).strip()
                report["image_id"] = subprocess.check_output(
                    ["docker", "inspect", container, "--format", "{{.Image}}"],
                    text=True,
                ).strip()
                output.write_text(json.dumps(report, indent=2) + "\n")
                print(json.dumps(report, indent=2))
            finally:
                subprocess.run(
                    command + ["down", "--volumes", "--remove-orphans"],
                    env=env,
                    stdout=log,
                    stderr=log,
                    check=True,
                )


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--image", required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    run(args.source.resolve(), args.image, args.output.resolve())
