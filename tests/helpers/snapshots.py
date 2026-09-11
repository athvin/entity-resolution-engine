"""Read a stage's DuckLake snapshot range from `run_stages` at runtime (S5.2, M22).

T-SNAP-1's recovery story is that `run_stages.snapshot_end` is a usable time-travel
handle. A test must obtain that handle from the lake at runtime, never from a committed
integer: a snapshot id is a property of the run that happened, so an absolute literal in
a test passes on the run that captured it and fails on every other (S4's preamble forbids
asserting snapshot counts for the same reason). These helpers are the one reader; a test
that needs `golden_records AT (VERSION => ...)` gets the version from here.
"""

from __future__ import annotations

import duckdb

from er.lake.model import SCHEMA_QUALIFIER

__all__ = ["UnknownStageError", "snapshot_end_for", "snapshot_range_for"]

_RUN_STAGES = f"{SCHEMA_QUALIFIER}.run_stages"


class UnknownStageError(LookupError):
    """No `run_stages` row carries a snapshot range for a `(run_id, stage)` pair.

    A named error rather than a bare `None` or `KeyError`, so a test asking for a stage
    that never ran (a typo'd stage name, a run that failed before committing) fails with
    a message naming the pair rather than a `TypeError` three lines later.
    """


def snapshot_range_for(
    connection: duckdb.DuckDBPyConnection, run_id: str, stage: str
) -> tuple[int, int]:
    """Return `(snapshot_start, snapshot_end)` for a run's stage, read from `run_stages`.

    The latest attempt wins when a stage has more than one `run_stages` row (a retry
    bumps `seq`), so the range is the one the stage committed under.

    Raises:
        UnknownStageError: no row for `(run_id, stage)`, or its range columns are NULL
            (a stage that was recorded but committed no snapshot).
    """
    row = connection.execute(
        f"SELECT snapshot_start, snapshot_end FROM {_RUN_STAGES} "
        "WHERE run_id = ? AND stage = ? ORDER BY seq DESC LIMIT 1",
        [run_id, stage],
    ).fetchone()
    if row is None or row[0] is None or row[1] is None:
        raise UnknownStageError(
            f"{_RUN_STAGES} has no snapshot range for run_id={run_id!r}, stage={stage!r}"
        )
    return int(row[0]), int(row[1])


def snapshot_end_for(connection: duckdb.DuckDBPyConnection, run_id: str, stage: str) -> int:
    """The `snapshot_end` of a run's stage — the version its writes are visible at."""
    return snapshot_range_for(connection, run_id, stage)[1]
