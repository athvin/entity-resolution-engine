"""Exercise real CLI failure, recovery and profiling on the committed small corpus."""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
from pathlib import Path

import duckdb
from ulid import ULID

from er.config.loader import load_config
from er.lake.ducklake import attach_statements, connect, detach

ROOT = Path(__file__).resolve().parents[2]


def test_profiled_pipeline_failure_can_resume_with_complete_logging(
    initialised_lake: duckdb.DuckDBPyConnection, tmp_path: Path
) -> None:
    sys.path.insert(0, str(ROOT / "benchmarks"))
    from profile_pipeline import check_fixture, install_fixture

    environment = {**os.environ, "ER_PROFILE_DIR": str(tmp_path / "profile"), "ER_PROFILE_SQL": "1"}
    subprocess.run(["dbt", "deps", "--project-dir", "dbt"], check=True, capture_output=True)
    run_id = str(ULID())
    drop = tmp_path / "drop"
    for source in ("crm", "billing", "webforms"):
        (drop / source).mkdir(parents=True)
        shutil.copy2(
            ROOT / f"fixtures/static/base_10/base/{source}.csv", drop / source / f"{source}.csv"
        )

    def invoke(*args: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            ["er", *args, "--json"], env=environment, capture_output=True, text=True
        )

    # The harness connection must not remain attached across a dbt subprocess.
    detach(initialised_lake)
    try:
        for source in ("crm", "billing", "webforms"):
            completed = invoke(
                "ingest", "--source", source, "--path", str(drop), "--run-id", run_id
            )
            assert completed.returncode == 0, completed.stderr
        failed = invoke("run-all", "--mode", "full", "--skip-ingest", "--run-id", run_id)
        assert failed.returncode not in (0, 10), failed.stderr
        with connect() as connection:
            prefix = connection.execute(
                "SELECT ended_at FROM lake.main.run_stages WHERE run_id=? AND stage='standardize'",
                [run_id],
            ).fetchone()
            assert connection.execute(
                "SELECT status FROM lake.main.runs WHERE run_id=?", [run_id]
            ).fetchone() == ("failed",)
        install_fixture(load_config(Path(os.environ["ER_CONFIG"])))
        resumed = invoke("run-all", "--mode", "full", "--resume", run_id, "--skip-ingest")
        assert resumed.returncode == 0, resumed.stderr
        manifests = [json.loads(line) for line in resumed.stdout.splitlines()]
        assert manifests[-1]["stages"] == 3
        with connect() as connection:
            check_fixture(connection)
            assert (
                connection.execute(
                    "SELECT ended_at FROM lake.main.run_stages "
                    "WHERE run_id=? AND stage='standardize'",
                    [run_id],
                ).fetchone()
                == prefix
            )
        noop = invoke("standardize", "--changed-only")
        assert noop.returncode == 10, noop.stderr
        records = [
            json.loads(line)
            for path in (tmp_path / "profile").glob("events-*.jsonl")
            for line in path.read_text().splitlines()
        ]
        assert not [record for record in records if record["event"].endswith("_error")]
        assert any(
            record["event"] == "span_end"
            and record["name"] == "match"
            and record["status"] == "failed"
            for record in records
        )
        assert any(
            record["event"] == "sql_profile" and record.get("dbt_model") for record in records
        )
    finally:
        for statement in attach_statements():
            initialised_lake.execute(statement)
