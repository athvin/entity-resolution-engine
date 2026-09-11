"""Unit arm of T-SNAP-1's snapshot reader (S5.2): the range helper over hand-built rows.

`snapshot_range_for` reads `run_stages` at `lake.main`, so this builds that relation in
an in-memory DuckDB attached as `lake` — no Compose substrate — and checks the two things
the integration test relies on but cannot isolate: a known `(run_id, stage)` returns an
ordered range, and an unknown one raises the named `UnknownStageError` rather than handing
back a `None` that explodes later.
"""

from __future__ import annotations

import duckdb
import pytest
from helpers.snapshots import UnknownStageError, snapshot_end_for, snapshot_range_for


def _lake_with_run_stages() -> duckdb.DuckDBPyConnection:
    connection = duckdb.connect(":memory:")
    connection.execute("ATTACH ':memory:' AS lake")  # its `main` schema already exists
    connection.execute(
        "CREATE TABLE lake.main.run_stages "
        "(run_id VARCHAR, stage VARCHAR, seq INTEGER, snapshot_start BIGINT, snapshot_end BIGINT)"
    )
    connection.execute(
        "INSERT INTO lake.main.run_stages VALUES "
        "('R1', 'assemble', 1, 40, 42), "
        "('R1', 'match', 1, 37, 38), "
        "('R1', 'assemble', 2, 50, 53)"  # a retry: higher seq wins
    )
    return connection


def test_snapshot_range_returns_ordered_range() -> None:
    connection = _lake_with_run_stages()
    start, end = snapshot_range_for(connection, "R1", "match")
    assert (start, end) == (37, 38)
    assert start <= end
    # The latest seq wins for a stage that was retried.
    assert snapshot_range_for(connection, "R1", "assemble") == (50, 53)
    assert snapshot_end_for(connection, "R1", "assemble") == 53


def test_snapshot_range_rejects_unknown_stage() -> None:
    connection = _lake_with_run_stages()
    with pytest.raises(UnknownStageError) as raised:
        snapshot_range_for(connection, "R1", "reconcile")
    assert "reconcile" in str(raised.value) and "R1" in str(raised.value)

    with pytest.raises(UnknownStageError):
        snapshot_range_for(connection, "does-not-exist", "assemble")
