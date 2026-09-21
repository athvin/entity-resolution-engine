import io
import itertools

import duckdb
import pytest

from er.lake.model import REGISTRY, create_table_sql
from er.obs import runctx


def test_reopened_run_preserves_initial_snapshot_and_updates_lifecycle_mode(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    connection = duckdb.connect(":memory:")
    connection.execute("ATTACH ':memory:' AS lake")
    for name in ("runs", "run_stages"):
        connection.execute(create_table_sql(REGISTRY[name]))
    snapshots = itertools.count()
    monkeypatch.setattr(runctx, "current_snapshot", lambda _connection: next(snapshots))

    def context(mode: str) -> runctx.RunContext:
        return runctx.RunContext(
            run_id="test-run",
            tenant="test",
            mode=mode,
            config_hash="hash",
            std_version="1",
            survivorship_version="1",
            source=runctx.held(connection),
            stream=io.StringIO(),
        )

    try:
        with context("stage") as run:
            with run.stage("ingest") as stage:
                stage.finish(0)
        first = connection.execute("SELECT snapshot_start FROM lake.main.runs").fetchone()[0]
        with context("full") as run:
            with run.stage("standardize") as stage:
                stage.finish(0)
        mode, lower, upper = connection.execute(
            "SELECT mode, snapshot_start, snapshot_end FROM lake.main.runs"
        ).fetchone()
        assert mode == "full"
        assert lower == first
        assert upper > lower
    finally:
        connection.close()
