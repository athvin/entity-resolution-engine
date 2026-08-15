"""The retention guard, without a lake (DesignDoc.md S4.7, S8.4).

S4.7 offers exactly one recovery tool — time travel over the `run_stages` snapshot
range — and one guarantee protecting it: "snapshot expiry in `er lake maintain`
never reaps a snapshot referenced by a `run_stages` row inside the retention
window". :func:`~er.lake.maintain.retention_cutoff` is that guarantee expressed as
a pure function, and this file is where its boundary is pinned, because a boundary
asserted only through a live lake is asserted only for the snapshots that lake
happened to hold.

The three cases are the three ways the cutoff can be wrong: it can reap a snapshot
a live run still points at, it can disagree with itself about the exact
``retain_days`` boundary, and it can take the wrong side of the ``min()``.
"""

from __future__ import annotations

from datetime import UTC, datetime, timedelta

from er.lake.maintain import ReferencedSnapshot, retention_cutoff

#: A fixed "now". Fixed rather than :func:`datetime.now`, so a boundary that is
#: only right for some instants fails every time rather than once a day.
NOW = datetime(2026, 8, 15, 12, 0, 0, tzinfo=UTC)

#: S4.0's default retention.
RETAIN_DAYS = 7


def days_ago(days: float) -> datetime:
    """The instant ``days`` before :data:`NOW`."""
    return NOW - timedelta(days=days)


def test_referenced_snapshot_inside_window_is_never_expirable() -> None:
    """AC4: a run inside the window pins its snapshot, however old the snapshot is.

    The snapshot here is 30 days old and the run that recorded it is 6 days old,
    which is the case the guard exists for: `run_stages.snapshot_start` is the
    version that was current *before* a stage began, so a quiet lake hands a recent
    run a long-dead snapshot id, and expiring by age alone would break exactly the
    range S4.7 tells an operator to travel to.
    """
    referenced = ReferencedSnapshot(run_started_at=days_ago(6), snapshot_at=days_ago(30))

    cutoff = retention_cutoff(NOW, RETAIN_DAYS, [referenced])

    # `older_than => cutoff` is exclusive on the pinned engine, so a snapshot AT the
    # cutoff survives: the guard pulls the cutoff back behind the window rather than
    # trusting the window alone.
    assert cutoff == referenced.snapshot_at
    assert cutoff < days_ago(RETAIN_DAYS)


def test_retain_days_boundary_is_inclusive() -> None:
    """AC4: a run started exactly ``retain_days`` ago is inside the window.

    Both arms in one test, because the boundary is one decision: the run at exactly
    ``retain_days`` still pins its snapshot, and the run a day older does not.
    """
    on_boundary = ReferencedSnapshot(run_started_at=days_ago(RETAIN_DAYS), snapshot_at=days_ago(40))
    outside = ReferencedSnapshot(run_started_at=days_ago(RETAIN_DAYS + 1), snapshot_at=days_ago(41))

    assert retention_cutoff(NOW, RETAIN_DAYS, [on_boundary]) == on_boundary.snapshot_at
    # The run outside the window constrains nothing, so the cutoff is the window
    # itself and its snapshot is expirable.
    assert retention_cutoff(NOW, RETAIN_DAYS, [outside]) == days_ago(RETAIN_DAYS)
    assert outside.snapshot_at < retention_cutoff(NOW, RETAIN_DAYS, [outside])


def test_cutoff_is_min_of_window_and_referenced() -> None:
    """AC4: the cutoff is the minimum, and both sides of it can win.

    With no references at all it is the window; with a reference newer than the
    window it is still the window, because retaining *more* than the window asked
    for is not what the guard does; with an older reference it is the reference.
    """
    assert retention_cutoff(NOW, RETAIN_DAYS, []) == days_ago(RETAIN_DAYS)

    newer = ReferencedSnapshot(run_started_at=days_ago(1), snapshot_at=days_ago(2))
    assert retention_cutoff(NOW, RETAIN_DAYS, [newer]) == days_ago(RETAIN_DAYS)

    older = ReferencedSnapshot(run_started_at=days_ago(1), snapshot_at=days_ago(12))
    assert retention_cutoff(NOW, RETAIN_DAYS, [newer, older]) == older.snapshot_at

    # `--retain-days 0` retains only what is referenced: the window collapses onto
    # `now`, which is what makes an operator-invoked reclamation of everything
    # unreferenced expressible without a second flag.
    assert retention_cutoff(NOW, 0, []) == NOW
    assert retention_cutoff(NOW, 0, [newer]) == NOW
