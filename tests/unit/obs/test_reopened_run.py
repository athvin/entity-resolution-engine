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


def test_correction_failure_cannot_erase_committed_snapshot(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    """A crash between the freeze commit and in-memory assignment preserves the journal."""
    with duckdb.connect() as connection:
        connection.execute("ATTACH ':memory:' AS lake")
        for name in ("runs", "run_stages"):
            connection.execute(create_table_sql(REGISTRY[name]))
        monkeypatch.setattr(runctx, "current_snapshot", lambda _: 1)
        with pytest.raises(RuntimeError, match="interrupted after freeze"):
            with runctx.RunContext(
                run_id="correction",
                mode="correction_pass",
                tenant="test",
                config_hash="hash",
                std_version="1",
                survivorship_version="1",
                model_version="model",
                source=runctx.held(connection),
                stream=io.StringIO(),
            ):
                connection.execute(
                    "UPDATE lake.main.runs SET tf_snapshot_id='frozen' WHERE run_id='correction'"
                )
                raise RuntimeError("interrupted after freeze")
        assert connection.execute(
            "SELECT status, model_version, tf_snapshot_id FROM lake.main.runs"
        ).fetchone() == ("failed", "model", "frozen")
