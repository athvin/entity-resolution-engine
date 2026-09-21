"""Prepare actual records and a model for CLI orchestration integration tests."""

from __future__ import annotations

import os
import shutil
import subprocess
import sys
from pathlib import Path

import duckdb
from ulid import ULID

from er.config.loader import load_config
from er.lake.ducklake import attach_statements, detach


def prepare_cli_fixture(connection: duckdb.DuckDBPyConnection, root: Path) -> None:
    project = Path(__file__).resolve().parents[2]
    sys.path.insert(0, str(project / "benchmarks"))
    from profile_pipeline import install_fixture

    detach(connection)
    try:
        subprocess.run(["dbt", "deps", "--project-dir", "dbt"], capture_output=True, check=True)
        install_fixture(load_config(Path(os.environ["ER_CONFIG"])))
        run_id = str(ULID())
        for source in ("crm", "billing", "webforms"):
            (root / source).mkdir(parents=True, exist_ok=True)
            shutil.copy2(
                project / f"fixtures/static/base_10/base/{source}.csv",
                root / source / f"{source}.csv",
            )
            result = subprocess.run(
                ["er", "ingest", "--source", source, "--path", str(root), "--run-id", run_id],
                capture_output=True,
                text=True,
            )
            assert result.returncode == 0, result.stdout + result.stderr
        result = subprocess.run(
            ["er", "standardize", "--run-id", run_id], capture_output=True, text=True
        )
        assert result.returncode == 0, result.stdout + result.stderr
    finally:
        for statement in attach_statements():
            connection.execute(statement)
