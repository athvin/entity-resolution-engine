"""`er lake maintain` against a real lake (DesignDoc.md S3, S4.0, S4.0b, S4.7, S5).

MINOR-lake-maint asks for three things and this file asserts each of them where it
can only be observed — on a substrate with a catalog, an object store and a
snapshot log:

* the three calls run in S3's order, which no unit test can see because the order
  is a property of what reached the engine;
* the command is a **writer**, so it takes the S4.0b tenant lock (exit ``3`` when
  it is held) and is recorded like one, with `runs.mode='maintain'` and one
  `run_stages` row;
* expiry never reaps a snapshot a `run_stages` row inside the retention window
  points at — S4.7's only recovery tool is time travel over that range, so this is
  asserted by travelling to every referenced snapshot *after* maintenance has run.

Every snapshot id here is captured at runtime from `run_stages`, per S8.1's blanket
rule: an absolute version would be right only for the run that produced it.
"""

from __future__ import annotations

import csv
import json
import os
import subprocess
from datetime import UTC, datetime, timedelta
from pathlib import Path
from typing import Any, cast

import duckdb
import pytest
from ulid import ULID

from er.lake.catalog import LOCK_HELD_MESSAGE, tenant_lock
from er.lake.ducklake import connect, current_snapshot
from er.lake.maintain import (
    CLEANUP_OLD_FILES,
    EXPIRE_SNAPSHOTS,
    MERGE_ADJACENT_FILES,
    maintain,
)
from er.lake.model import SCHEMA_QUALIFIER
from er.lake.objectstore import ObjectStore

#: `configs/test.yaml`'s tenant, which Compose supplies as `ER_CONFIG` (S7.1).
TENANT = "test"

#: S4.0's exit code for "advisory lock not acquired".
LOCK_CONFLICT_EXIT = 3

#: The source deliveries are written for, and the committed CSV whose header shape
#: they follow — so a change to the `crm` columns moves this file with it.
SOURCE = "crm"
REPO_ROOT = Path(__file__).resolve().parents[2]
CRM_HEADER = tuple(
    (REPO_ROOT / "fixtures" / "static" / "base_10" / "base" / "crm.csv")
    .read_text(encoding="utf-8")
    .splitlines()[0]
    .split(",")
)

#: Rows per delivery. It has to exceed DuckLake's data-inlining row limit — ten on
#: the pinned engine — or the delivery lands in the catalog as inlined data and the
#: `DATA_PATH` prefix stays empty, which would make AC6's reclamation unobservable
#: and every maintenance count a zero for the wrong reason.
DELIVERY_ROWS = 50


def write_delivery(root: Path, *, start: int) -> Path:
    """Write a `crm` delivery of :data:`DELIVERY_ROWS` distinct keys under ``root``.

    ``--path`` is the drop-folder ROOT and S4.1 reads ``<root>/<source>/``, so the
    CSV goes in a `crm/` directory beneath it. ``start`` offsets the key range, so
    two deliveries under one namespace are two appends rather than a re-ingest.
    """
    directory = root / SOURCE
    directory.mkdir(parents=True, exist_ok=True)
    with (directory / "crm.csv").open("w", newline="", encoding="utf-8") as handle:
        writer = csv.DictWriter(handle, fieldnames=CRM_HEADER)
        writer.writeheader()
        for index in range(start, start + DELIVERY_ROWS):
            writer.writerow(
                {
                    "crm_id": f"C{index:05d}",
                    "first_name": f"Given{index}",
                    "last_name": f"Family{index}",
                    "email_address": f"person{index}@example.com",
                    "phone": f"(628) 555-{index % 10000:04d}",
                    "street_address": f"{index} Judah Street",
                    "city": "San Francisco",
                    "state": "CA",
                    "zip": "94107",
                    "dob": "1985-03-14",
                    "last_modified": "2024-01-05 08:12:00",
                    "persona_id": f"P{index}",
                }
            )
    return root


def ingest(root: Path) -> None:
    """Land the delivery under ``root``, refusing to continue if it did not."""
    result = run_er("ingest", "--source", SOURCE, "--path", str(root))
    assert result.returncode == 0, result.stdout + result.stderr


@pytest.fixture
def delivery(tmp_path: Path) -> Path:
    """A drop-folder root holding one `crm` delivery."""
    return write_delivery(tmp_path / "delivery", start=0)


def run_er(*args: str) -> subprocess.CompletedProcess[str]:
    """Invoke the installed `er` console script in this session's namespace."""
    return subprocess.run(
        ["er", *args], capture_output=True, text=True, env=dict(os.environ), check=False
    )


def maintain_counts(*args: str) -> tuple[subprocess.CompletedProcess[str], dict[str, int]]:
    """Run `er lake maintain --json` and return it with the counts it printed."""
    result = run_er("lake", "maintain", "--json", *args)
    assert result.returncode == 0, result.stdout + result.stderr
    return result, json.loads(result.stdout.strip())


def referenced_versions(connection: duckdb.DuckDBPyConnection, within_days: int) -> list[int]:
    """Every snapshot version a `run_stages` row of a recent run points at.

    Read at runtime, both bounds of the range, exactly as S8.1 requires — and with
    the window applied to `runs.started_at`, because "inside the retention window"
    is a statement about the run, not about the snapshot it recorded.
    """
    horizon = datetime.now(UTC).replace(tzinfo=None) - timedelta(days=within_days)
    rows = connection.execute(
        f"""
        SELECT DISTINCT version FROM (
            SELECT stage.snapshot_start AS version, run.started_at AS started_at
            FROM {SCHEMA_QUALIFIER}.run_stages AS stage
            JOIN {SCHEMA_QUALIFIER}.runs AS run ON run.run_id = stage.run_id
            UNION ALL
            SELECT stage.snapshot_end AS version, run.started_at AS started_at
            FROM {SCHEMA_QUALIFIER}.run_stages AS stage
            JOIN {SCHEMA_QUALIFIER}.runs AS run ON run.run_id = stage.run_id
        )
        WHERE version IS NOT NULL AND started_at >= ?
        ORDER BY version
        """,
        [horizon],
    ).fetchall()
    return [int(version) for (version,) in rows]


class RecordingConnection:
    """A lake connection that records the SQL it is asked to run, then runs it.

    A proxy rather than a fake: the statements still reach a real attached lake, so
    what the order assertion reads is the sequence the engine actually saw. A fake
    would prove only that this file and :mod:`er.lake.maintain` agree.
    """

    def __init__(self, connection: duckdb.DuckDBPyConnection) -> None:
        self.connection = connection
        self.calls: list[str] = []

    def execute(self, statement: str, parameters: Any = None) -> Any:
        self.calls.append(statement)
        if parameters is None:
            return self.connection.execute(statement)
        return self.connection.execute(statement, parameters)

    def maintenance_calls(self) -> list[str]:
        """The DuckLake maintenance calls issued, in issue order."""
        return [call for call in self.calls if "ducklake_" in call]


def test_calls_run_in_specified_order(initialised_lake: duckdb.DuckDBPyConnection) -> None:
    """AC1: merge → expire → cleanup, then exit 0 printing the three counts.

    The order is S3's and it is normative: cleaning before expiry deletes nothing,
    because a file is unreferenced only once the snapshot referencing it is gone,
    and expiring before merging leaves the small files compaction was for.
    """
    # A connection of its own rather than the session handle: the rows maintenance
    # reads were written by other processes, and a cached handle answers with what
    # this session last saw.
    with connect() as connection:
        recorder = RecordingConnection(connection)
        result = maintain(cast(duckdb.DuckDBPyConnection, recorder), 7)

    assert recorder.maintenance_calls() == [
        MERGE_ADJACENT_FILES,
        EXPIRE_SNAPSHOTS,
        CLEANUP_OLD_FILES,
    ]
    # The guard is read before the first call: `run_stages` has to be read against
    # the snapshot log as it stood when maintenance began, and the calls commit.
    assert recorder.calls.index(MERGE_ADJACENT_FILES) > 0
    assert result.files_merged >= 0

    # The same three counts through the command, which is where S4.0 puts them.
    printed = run_er("lake", "maintain")
    assert printed.returncode == 0, printed.stdout + printed.stderr
    line = printed.stdout.strip()
    assert line.startswith("files_merged="), line
    assert "snapshots_expired=" in line and "files_deleted=" in line


def test_lock_conflict_exits_3(initialised_lake: duckdb.DuckDBPyConnection) -> None:
    """AC2: maintenance can never run beside a pipeline stage (S4.0b, S4.7).

    The refusal is asserted to leave the snapshot log untouched, not merely to be
    non-zero: a maintenance run that had already merged or expired something before
    noticing the contention would have mutated a lake another writer was using.
    """
    holder = str(ULID())
    with connect() as connection:
        before = current_snapshot(connection)

    with tenant_lock(TENANT, run_id=holder):
        refused = run_er("lake", "maintain")

    assert refused.returncode == LOCK_CONFLICT_EXIT, refused.stdout + refused.stderr
    assert LOCK_HELD_MESSAGE.format(tenant=TENANT, run_id=holder) in refused.stderr
    with connect() as connection:
        assert current_snapshot(connection) == before
    assert refused.stdout == "", "a refused writer emitted command output"


def test_writes_maintain_run_and_stage_rows(
    initialised_lake: duckdb.DuckDBPyConnection,
) -> None:
    """AC3: `runs.mode='maintain'` and exactly one `stage='maintain'` row (S5.2).

    S4.0 calls this command a writer "recorded like one"; both enum values are
    already in S5's DDL, so a maintenance run that wrote no row would be the only
    writer whose snapshot range is unaddressable.
    """
    run_id = str(ULID())
    result = run_er("lake", "maintain", "--run-id", run_id)
    assert result.returncode == 0, result.stdout + result.stderr

    with connect() as connection:
        assert connection.execute(
            f"SELECT mode, status FROM {SCHEMA_QUALIFIER}.runs WHERE run_id = ?", [run_id]
        ).fetchall() == [("maintain", "succeeded")]
        rows = connection.execute(
            f"""
            SELECT stage, status, snapshot_start, snapshot_end, duration_ms
            FROM {SCHEMA_QUALIFIER}.run_stages WHERE run_id = ?
            """,
            [run_id],
        ).fetchall()

    assert len(rows) == 1, rows
    stage, status, snapshot_start, snapshot_end, duration_ms = rows[0]
    assert (stage, status) == ("maintain", "succeeded")
    assert snapshot_start is not None and snapshot_end is not None
    assert duration_ms is not None


def test_referenced_snapshots_remain_time_travelable(
    initialised_lake: duckdb.DuckDBPyConnection,
    delivery: Path,
) -> None:
    """AC5: the retention guard keeps S4.7's one recovery tool usable.

    The failure this exists for is a maintenance run that expires by age alone —
    the teardown spelling, ``older_than => now()`` — which reaps every snapshot a
    live run points at and leaves an operator holding a range they cannot read.
    """
    ingest(delivery)
    assert run_er("run-all", "--mode", "incremental", "--skip-ingest").returncode == 0

    _, counts = maintain_counts("--retain-days", "7")

    with connect() as connection:
        versions = referenced_versions(connection, within_days=7)
        assert versions, "no run_stages row recorded a snapshot to travel to"
        for version in versions:
            connection.execute(
                f"SELECT * FROM {SCHEMA_QUALIFIER}.raw_records AT (VERSION => {version})"
            ).fetchall()

    # Nothing this young is outside a seven-day window, so the guard is not merely
    # the reason the reads above succeeded — the window is too.
    assert counts["snapshots_expired"] == 0


def test_expiry_reclaims_prefix_objects(
    initialised_lake: duckdb.DuckDBPyConnection,
    object_store: ObjectStore,
    tmp_path: Path,
) -> None:
    """AC6: expiring out-of-window snapshots strictly shrinks the `DATA_PATH` prefix.

    ``--retain-days 0`` collapses the window onto now, so every snapshot a run
    recorded before this instant is out of window and expirable — the only way to
    reach the reclamation path in a test, since a snapshot cannot be backdated.

    Three deliveries, because reclamation needs something to reclaim: each lands as
    its own data file, compaction rewrites the three as one, and the originals
    become unreferenced only once the snapshots that held them are expired. That is
    the whole S3 sequence observed end to end, in the one place it is visible.
    """
    for batch in range(3):
        ingest(write_delivery(tmp_path / f"batch{batch}", start=batch * DELIVERY_ROWS))

    data_path = os.environ["ER_LAKE_DATA_PATH"]
    before = object_store.list_prefix(data_path)
    assert len(before) > 1, "the deliveries were inlined; there is nothing to reclaim"

    _, counts = maintain_counts("--retain-days", "0")

    assert counts["files_merged"] > 0, counts
    assert counts["snapshots_expired"] > 0, counts
    assert counts["files_deleted"] > 0, counts
    assert len(object_store.list_prefix(data_path)) < len(before)


def test_second_invocation_is_a_zero_count_noop(
    initialised_lake: duckdb.DuckDBPyConnection,
    object_store: ObjectStore,
    delivery: Path,
) -> None:
    """AC7, and AC6's other arm: a second run exits 0 with nothing left to reclaim.

    Both runs are at S4.0's default retention, which is what "twice in a row" means
    for a lake minutes old: nothing is out of a seven-day window, so nothing is
    expirable and nothing becomes unreferenced. S4.0's table gives this command no
    exit ``10``, so that is a successful run reporting zeros rather than a refusal.

    AC6's "unchanged when none was expired" arm is asserted as *nothing was
    reclaimed* — no object present before the second run is missing after it —
    because compaction may legitimately write a new merged file, and an equality on
    the count would then be an assertion about `merge_adjacent_files`' heuristics
    rather than about expiry.
    """
    ingest(delivery)

    _, first = maintain_counts()
    assert first["snapshots_expired"] == 0, first

    data_path = os.environ["ER_LAKE_DATA_PATH"]
    between = set(object_store.list_prefix(data_path))

    _, second = maintain_counts()

    assert second["snapshots_expired"] == 0, second
    assert second["files_deleted"] == 0, second
    assert between <= set(object_store.list_prefix(data_path))
