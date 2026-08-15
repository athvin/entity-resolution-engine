"""`er lake maintain`: compaction, expiry and cleanup (DesignDoc.md S3, S4.0, S4.7).

MINOR-lake-maint is that a lake which commits a snapshot range per stage
accumulates small files and snapshot history without bound, and a reused local
volume grows forever. This module is the reclamation path, and S3 fixes its
order: :data:`MERGE_ADJACENT_FILES` → :data:`EXPIRE_SNAPSHOTS` →
:data:`CLEANUP_OLD_FILES`. The order is normative rather than tidy — cleaning
before expiry deletes nothing, because a file is only unreferenced once the
snapshot that referenced it is gone, and expiring before merging leaves behind
exactly the small files compaction was meant to remove.

Three rules govern it, and each is a decision the spec makes:

* **Expiry never reaps a snapshot the recovery path still needs.** S4.7 offers
  time travel over the `run_stages` snapshot range as the *only* recovery tool and
  no rollback at all, so "snapshot expiry in `er lake maintain` never reaps a
  snapshot referenced by a `run_stages` row inside the retention window" is what
  keeps that tool usable. The guard is therefore not "retain N days of snapshots":
  it is a ``min()`` over the window and the oldest referenced snapshot, computed
  by :func:`retention_cutoff` — a pure function, so the boundary is testable
  without a lake.
* **Maintenance is a writer.** It runs under the same S4.0b tenant advisory lock
  every other writer takes, so it can never run concurrently with a pipeline stage
  (S4.7). The lock is taken by the CLI, outside the `runs` row, exactly as it is
  for every other mutating command; this module does the work inside it and takes
  no lock of its own, because a second acquisition on a second session would
  contend with the first.
* **A second run is a no-op, not an error.** S4.0's table gives this command no
  exit ``10``, so an invocation with nothing left to reclaim exits ``0`` with zero
  counts.

The three calls are spelled with the alias as their first positional argument
rather than catalog-scoped. That is forced by the pinned ``duckdb==1.5.5``
ducklake extension, in which ``CALL lake.expire_snapshots(…)`` raises
``Catalog Error: Table Function with name expire_snapshots does not exist!``
because DuckDB reads ``lake.`` as a schema qualifier; `tests/conftest.py` already
carries the same spelling for the S8.1 teardown.
"""

from __future__ import annotations

from collections.abc import Iterable, Sequence
from dataclasses import dataclass
from datetime import UTC, datetime, timedelta
from typing import Final

import duckdb

from er.lake.ducklake import LAKE_ALIAS
from er.lake.model import SCHEMA_QUALIFIER

__all__ = [
    "CLEANUP_OLD_FILES",
    "DEFAULT_RETAIN_DAYS",
    "EXPIRE_SNAPSHOTS",
    "MERGE_ADJACENT_FILES",
    "MaintainResult",
    "ReferencedSnapshot",
    "maintain",
    "referenced_snapshots",
    "retention_cutoff",
]

#: ``--retain-days`` when the operator supplies none (S4.0).
DEFAULT_RETAIN_DAYS: Final[int] = 7

#: S3's three calls, in S3's order. Spelled once each, so the sequence a lake sees
#: is the sequence this module documents.
MERGE_ADJACENT_FILES: Final[str] = f"CALL ducklake_merge_adjacent_files('{LAKE_ALIAS}')"
EXPIRE_SNAPSHOTS: Final[str] = f"CALL ducklake_expire_snapshots('{LAKE_ALIAS}', older_than => ?)"
CLEANUP_OLD_FILES: Final[str] = (
    f"CALL ducklake_cleanup_old_files('{LAKE_ALIAS}', cleanup_all => true)"
)

# `ducklake_merge_adjacent_files` returns one row per compacted table, with the
# columns (schema_name, table_name, files_processed, files_created). The count S4.0
# prints is the number of files that were merged away, which is `files_processed`.
_FILES_PROCESSED: Final[int] = 2

# Every snapshot version a `run_stages` row points at, with the instant the run
# owning that row started. Both bounds of the range are referenced — S4 makes the
# range the unit of time travel, so reaping either end would leave a range whose
# endpoints are no longer readable. NULL bounds are a stage that committed nothing
# and reference no snapshot at all.
_REFERENCED_SQL: Final[str] = f"""
WITH referenced(version, run_started_at) AS (
    SELECT stage.snapshot_start, run.started_at
    FROM {SCHEMA_QUALIFIER}.run_stages AS stage
    JOIN {SCHEMA_QUALIFIER}.runs AS run ON run.run_id = stage.run_id
    WHERE stage.snapshot_start IS NOT NULL
    UNION ALL
    SELECT stage.snapshot_end, run.started_at
    FROM {SCHEMA_QUALIFIER}.run_stages AS stage
    JOIN {SCHEMA_QUALIFIER}.runs AS run ON run.run_id = stage.run_id
    WHERE stage.snapshot_end IS NOT NULL
)
SELECT referenced.run_started_at, snapshot.snapshot_time
FROM referenced
JOIN {LAKE_ALIAS}.snapshots() AS snapshot ON snapshot.snapshot_id = referenced.version
"""


@dataclass(frozen=True, slots=True)
class ReferencedSnapshot:
    """One snapshot a `run_stages` row points at, and when its run started.

    Both instants are needed and neither substitutes for the other: the retention
    *window* is about when the run started, and what the guard must not reap is the
    snapshot that run recorded — which may be far older, because
    ``run_stages.snapshot_start`` is the version that was current before the stage
    began and a quiet lake produces no new one for as long as it stays quiet.
    """

    run_started_at: datetime
    snapshot_at: datetime


@dataclass(frozen=True, slots=True)
class MaintainResult:
    """What one `er lake maintain` reclaimed, in the terms S4.0's stdout names."""

    files_merged: int
    snapshots_expired: int
    files_deleted: int


def retention_cutoff(
    now: datetime, retain_days: int, referenced: Iterable[ReferencedSnapshot]
) -> datetime:
    """The instant before which a snapshot may be expired (S4.7).

    The minimum of two bounds, because either alone is wrong: ``now -
    retain_days`` alone would reap a snapshot that a run inside the window still
    points at — S4.7's one recovery tool reading a range whose start no longer
    exists — and the oldest referenced snapshot alone would retain history forever
    on a lake nobody has run anything against.

    The bound is exclusive on both sides, which is what makes AC4's boundary
    statements agree: a run started *exactly* ``retain_days`` ago is inside the
    window, and ``older_than => cutoff`` retains a snapshot whose time is exactly
    the cutoff (verified against the pinned engine).

    Args:
        now: the instant the maintenance run is reckoned from.
        retain_days: ``--retain-days``; ``0`` retains only what is referenced.
        referenced: every ``(run_started_at, snapshot_at)`` pair `run_stages`
            currently points at, in any order.

    Returns:
        The cutoff, in the same time zone convention as ``now``. Every datetime
        passed in must share that convention — naive or aware — since Python will
        not order a mixture, and :func:`referenced_snapshots` normalises to UTC.
    """
    window_start = now - timedelta(days=retain_days)
    inside_window = [
        reference.snapshot_at
        for reference in referenced
        if reference.run_started_at >= window_start
    ]
    return min([window_start, *inside_window])


def referenced_snapshots(connection: duckdb.DuckDBPyConnection) -> tuple[ReferencedSnapshot, ...]:
    """Every live snapshot a `run_stages` row points at, with its run's start.

    A version no longer in the snapshot log — one an earlier maintenance run
    already expired — simply does not join, and so constrains nothing.

    Both columns are normalised to aware UTC: `runs.started_at` is a `TIMESTAMP`
    (S5 permits no `TIMESTAMPTZ`) and reads back naive, while DuckLake's
    ``snapshot_time`` is a ``TIMESTAMP WITH TIME ZONE``, and a comparison between
    the two would raise rather than answer.
    """
    rows: Sequence[tuple[datetime, datetime]] = connection.execute(_REFERENCED_SQL).fetchall()
    return tuple(
        ReferencedSnapshot(run_started_at=_utc(started_at), snapshot_at=_utc(snapshot_at))
        for started_at, snapshot_at in rows
    )


def maintain(
    connection: duckdb.DuckDBPyConnection,
    retain_days: int = DEFAULT_RETAIN_DAYS,
    *,
    now: datetime | None = None,
) -> MaintainResult:
    """Run S3's three maintenance calls, in S3's order, and count what they did.

    The cutoff is computed **before** the first call: the guard has to read
    `run_stages` against the snapshot log as it stands when maintenance begins, and
    the calls themselves commit snapshots.

    Args:
        connection: an attached lake connection, held by a caller that already owns
            the S4.0b tenant lock. This function takes no lock of its own.
        retain_days: ``--retain-days`` (S4.0; default 7).
        now: the instant the retention window is reckoned from; the clock by
            default. Supplied by a caller that needs a deterministic window.

    Returns:
        The three counts S4.0 prints. All zero for an idempotent second run.
    """
    moment = datetime.now(UTC) if now is None else now
    cutoff = retention_cutoff(moment, retain_days, referenced_snapshots(connection))

    merged = connection.execute(MERGE_ADJACENT_FILES).fetchall()
    expired = connection.execute(EXPIRE_SNAPSHOTS, [cutoff]).fetchall()
    deleted = connection.execute(CLEANUP_OLD_FILES).fetchall()

    return MaintainResult(
        files_merged=sum(int(row[_FILES_PROCESSED]) for row in merged),
        # Row counts, not a reported total: each function returns one row per
        # snapshot expired and per file deleted, and counting them here is the only
        # reading that cannot disagree with what the engine actually did.
        snapshots_expired=len(expired),
        files_deleted=len(deleted),
    )


def _utc(moment: datetime) -> datetime:
    """``moment`` as an aware UTC instant, treating a naive value as UTC.

    Naive-means-UTC is the convention `runs`/`run_stages` are written under
    (:mod:`er.obs.runctx` drops the offset at the boundary), so reading one back is
    the inverse of that write and not a guess.
    """
    if moment.tzinfo is None:
        return moment.replace(tzinfo=UTC)
    return moment.astimezone(UTC)
