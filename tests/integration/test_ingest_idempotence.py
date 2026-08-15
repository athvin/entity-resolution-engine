"""The ingest arm of T-IDEM-1, against the Compose substrate (S4, S4.1, S5.2).

The S4 preamble states the idempotency contract in the abstract: re-executing a
stage with the same key leaves the lake logically unchanged and exits `10`. `er
ingest` is the first stage that can be held to it, and its key is
`(source_system, source_record_id, content_hash)`. So every assertion here is about
what survived in the lake after a real `er` process wrote it, not about what a
function returned.

The command is spawned as a subprocess, exactly as Compose runs it (S7.1), and the
lake is read back on a connection of its own — the rows were committed by another
process, so what is asserted is what reached the catalog rather than what a cached
handle holds.

The M2 and M4 arms of T-IDEM-1 (`int_std_records`, `match_scores`,
`entity_membership`, `golden_*`) are deliberately absent: those relations do not
exist in M1, and they have their own node ids in `tests/integration/test_idempotence.py`.
"""

from __future__ import annotations

import csv
import json
import os
import subprocess
from pathlib import Path
from typing import Any

import duckdb
import pytest
from ulid import ULID

from er.config.loader import load_config
from er.errors import ExitCode
from er.ingest.hashing import content_hash
from er.lake.columns import VOLATILE_COLUMNS
from er.lake.ducklake import connect
from er.lake.model import SCHEMA_QUALIFIER
from er.obs.counters import DECLARED_COUNTERS

#: The source this suite delivers to; `configs/test.yaml` declares it as `csv`.
SOURCE = "crm"

#: `sources.crm`'s delivered header, in the order the fixture writes it. The mapped
#: columns are `configs/test.yaml`'s; `last_modified` is the `updated_at_column`, and
#: it is here precisely because it is NOT in `columns` -- S4.1 stores the full source
#: row in `payload` verbatim while hashing only the mapped columns, and a delivery
#: with no unmapped column could not tell the two apart.
HEADER = (
    "crm_id",
    "first_name",
    "last_name",
    "email_address",
    "phone",
    "street_address",
    "city",
    "state",
    "zip",
    "dob",
    "last_modified",
)

#: Five distinct keys. Small enough to assert on row by row, and it is the `N` every
#: acceptance criterion here is written in terms of.
BASE_ROWS: tuple[tuple[str, ...], ...] = (
    (
        "C1",
        "Ada",
        "Lovelace",
        "ada@example.com",
        "555-0100",
        "1 Analytical Way",
        "London",
        "LN",
        "10001",
        "1815-12-10",
        "2026-01-01T00:00:00",
    ),
    (
        "C2",
        "Grace",
        "Hopper",
        "grace@example.com",
        "555-0101",
        "2 Compiler St",
        "Arlington",
        "VA",
        "22201",
        "1906-12-09",
        "2026-01-01T00:00:00",
    ),
    (
        "C3",
        "Alan",
        "Turing",
        "alan@example.com",
        "555-0102",
        "3 Enigma Rd",
        "Wilmslow",
        "CH",
        "33101",
        "1912-06-23",
        "2026-01-01T00:00:00",
    ),
    (
        "C4",
        "Katherine",
        "Johnson",
        "katherine@example.com",
        "555-0103",
        "4 Orbit Ave",
        "Hampton",
        "VA",
        "23666",
        "1918-08-26",
        "2026-01-01T00:00:00",
    ),
    (
        "C5",
        "Edsger",
        "Dijkstra",
        "edsger@example.com",
        "555-0104",
        "5 Shortest Path",
        "Rotterdam",
        "ZH",
        "30001",
        "1930-05-11",
        "2026-01-01T00:00:00",
    ),
)

N = len(BASE_ROWS)

#: `raw_records` as S5 declares it, and the projection every read here uses. S5.1
#: forbids `SELECT *` across an additive change, so the columns are named once.
RAW_COLUMNS: tuple[str, ...] = (
    "source_system",
    "source_record_id",
    "payload",
    "content_hash",
    "is_deleted",
    "deleted_at",
    "ingest_batch_id",
    "ingested_at",
)

#: The positions a re-ingest must leave byte-identical. S5.0 defines
#: VOLATILE_COLUMNS once and forbids a second exclusion list, so this is the
#: relation's column list minus that set rather than a transcription of it.
STABLE_POSITIONS: tuple[int, ...] = tuple(
    index for index, column in enumerate(RAW_COLUMNS) if column not in VOLATILE_COLUMNS
)

#: AC6's key set: the S4.1.1 per-stage names plus the promoted counters this stage
#: writes (S5.2's completeness rule). Built from the declared tuple rather than
#: retyped, so a name added to S4.1.1 fails here instead of silently widening.
EXPECTED_COUNTER_KEYS = frozenset(DECLARED_COUNTERS["ingest"]) | {"rows_in", "rows_out"}


def run_er(*args: str) -> subprocess.CompletedProcess[str]:
    """Invoke the installed `er` console script in this session's namespace."""
    return subprocess.run(
        ["er", *args], capture_output=True, text=True, env=dict(os.environ), check=False
    )


def query(connection: duckdb.DuckDBPyConnection, sql: str, *parameters: Any) -> list[Any]:
    return connection.execute(sql, list(parameters)).fetchall()


def write_delivery(root: Path, rows: tuple[tuple[str, ...], ...]) -> Path:
    """Write ``rows`` as `<root>/crm/crm.csv` and return the drop-folder root.

    ``--path`` is the drop-folder ROOT: S4.1 reads `storage.drop_dir/<source>/`, and
    the flag overrides the root half of that.
    """
    directory = root / SOURCE
    directory.mkdir(parents=True, exist_ok=True)
    with (directory / "crm.csv").open("w", newline="", encoding="utf-8") as handle:
        writer = csv.writer(handle)
        writer.writerow(HEADER)
        writer.writerows(rows)
    return root


def hashed_columns() -> tuple[str, ...]:
    """`sources.crm.columns` as the source column names, in the declared order."""
    config = load_config(Path(os.environ["ER_CONFIG"]))
    return tuple(config.sources[SOURCE].columns.values())


def expected_hashes(rows: tuple[tuple[str, ...], ...]) -> dict[str, str]:
    """The S4.1 `content_hash` of each delivered row, recomputed from the CSV."""
    columns = hashed_columns()
    return {row[0]: content_hash(dict(zip(HEADER, row, strict=True)), columns) for row in rows}


def raw_records(connection: duckdb.DuckDBPyConnection) -> list[Any]:
    """Every `raw_records` row, in a total order that does not depend on write order."""
    return query(
        connection,
        f"SELECT {', '.join(RAW_COLUMNS)} FROM {SCHEMA_QUALIFIER}.raw_records "
        f"ORDER BY source_record_id, content_hash",
    )


def stable(rows: list[Any]) -> list[tuple[Any, ...]]:
    """``rows`` with every VOLATILE_COLUMNS value dropped (S5.0)."""
    return [tuple(row[index] for index in STABLE_POSITIONS) for row in rows]


def batches(connection: duckdb.DuckDBPyConnection) -> list[Any]:
    """Every `ingest_batches` row, oldest first (S5)."""
    return query(
        connection,
        f"SELECT ingest_batch_id, run_id, source_system, path, new_count, changed_count, "
        f"unchanged_count, tombstone_count, resurrected_count, full_refresh_keys, created_at "
        f"FROM {SCHEMA_QUALIFIER}.ingest_batches ORDER BY created_at, ingest_batch_id",
    )


def stage_records(stderr: str) -> list[dict[str, Any]]:
    """The S5.2 records on stderr, in emission order."""
    return [json.loads(line) for line in stderr.splitlines() if line.startswith("{")]


@pytest.fixture
def delivery(tmp_path: Path) -> Path:
    """A drop-folder root holding the five-key `crm` delivery."""
    return write_delivery(tmp_path, BASE_ROWS)


def test_first_delivery_appends_and_writes_manifest(
    initialised_lake: duckdb.DuckDBPyConnection, delivery: Path
) -> None:
    """AC1/AC4: N keys appended live, one manifest row, and the pinned hash."""
    result = run_er("ingest", "--source", SOURCE, "--path", str(delivery))

    assert result.returncode == int(ExitCode.SUCCESS), result.stdout + result.stderr

    with connect() as connection:
        rows = raw_records(connection)
        manifests = batches(connection)

    assert len(rows) == N, "S4.1: one appended row per delivered key"
    for _system, _record_id, payload, _hash, is_deleted, deleted_at, batch_id, ingested_at in rows:
        # AC1: this stage writes live content versions only. The tombstone arm that
        # writes `true` and a non-NULL `deleted_at` is S4.1.1's, and a later ticket's.
        assert is_deleted is False
        assert deleted_at is None
        assert batch_id and ingested_at is not None
        assert payload, "S4.1: payload is the full source row verbatim"

    # AC4: the stored hash is ER-029's function over `sources.crm.columns`, recomputed
    # here from the CSV rather than read back out of the row that is under test.
    expected = expected_hashes(BASE_ROWS)
    assert {row[1]: row[3] for row in rows} == expected

    # AC1: the payload is the FULL source row, including `last_modified`, which the
    # `columns` block does not name and the hash therefore excludes.
    payloads = {row[1]: json.loads(row[2]) for row in rows}
    assert set(payloads["C1"]) == set(HEADER)
    assert payloads["C1"]["last_modified"] == BASE_ROWS[0][-1]

    assert len(manifests) == 1, "S4.1: one `ingest_batches` row per invocation"
    (
        _batch_id,
        _run,
        source_system,
        path,
        new,
        changed,
        unchanged,
        tombstoned,
        resurrected,
        full_refresh,
        created_at,
    ) = manifests[0]
    assert source_system == SOURCE
    assert path == str(delivery)
    assert (new, changed, unchanged) == (N, 0, 0)
    assert (tombstoned, resurrected, full_refresh) == (0, 0, False)
    assert created_at is not None


def test_empty_delivery_exits_10_with_zero_appended_rows(
    initialised_lake: duckdb.DuckDBPyConnection, tmp_path: Path
) -> None:
    """AC5: a delivery with nothing parsable is `10`, and still writes its manifest.

    `10` and not `2`: the empty-delivery *refusal* of S4.1.1 is the
    `--full-refresh-keys` arm, where an empty file would destroy live keys. An
    ordinary delivery with no file is simply a stage with nothing to do, and it must
    not abort an `er run-all` chain.
    """
    result = run_er("ingest", "--source", SOURCE, "--path", str(tmp_path))

    assert result.returncode == int(ExitCode.NOTHING_TO_DO), result.stdout + result.stderr

    with connect() as connection:
        assert raw_records(connection) == []
        manifests = batches(connection)

    assert len(manifests) == 1, "S4.1: the manifest row is written on a no-op too"
    assert manifests[0][4:9] == (0, 0, 0, 0, 0)


def test_reingesting_a_delivery_appends_no_rows_and_exits_10(
    initialised_lake: duckdb.DuckDBPyConnection, delivery: Path
) -> None:
    """AC2: re-execution leaves the lake logically unchanged and exits `10`."""
    first = run_er("ingest", "--source", SOURCE, "--path", str(delivery))
    assert first.returncode == int(ExitCode.SUCCESS), first.stdout + first.stderr

    with connect() as connection:
        before = stable(raw_records(connection))

    second = run_er("ingest", "--source", SOURCE, "--path", str(delivery))

    assert second.returncode == int(ExitCode.NOTHING_TO_DO), second.stdout + second.stderr

    with connect() as connection:
        after = raw_records(connection)
        manifests = batches(connection)

    assert len(after) == N, "S4.1: a re-delivered identical file appends zero rows"
    # Not just the row count: every non-VOLATILE value of every surviving row is
    # byte-identical, which is what "logically unchanged" means in the S4 preamble.
    assert stable(after) == before

    assert len(manifests) == 2, "S4.1: every invocation persists a manifest row"
    _batch, _run, _source, _path, new, changed, unchanged, tombstoned = manifests[1][:8]
    # T-IDEM-1a's assertion, verbatim, plus the count that says the keys were seen.
    assert new == 0 and changed == 0 and tombstoned == 0
    assert unchanged == N


def test_changed_content_hash_appends_a_version_row(
    initialised_lake: duckdb.DuckDBPyConnection, delivery: Path
) -> None:
    """AC3: a corrected record ADDS a row; both versions survive (D7)."""
    first = run_er("ingest", "--source", SOURCE, "--path", str(delivery))
    assert first.returncode == int(ExitCode.SUCCESS), first.stdout + first.stderr

    corrected = tuple(
        (row[0], row[1], "Byron", *row[3:]) if row[0] == "C1" else row for row in BASE_ROWS
    )
    write_delivery(delivery, corrected)

    result = run_er("ingest", "--source", SOURCE, "--path", str(delivery))

    assert result.returncode == int(ExitCode.SUCCESS), result.stdout + result.stderr

    with connect() as connection:
        rows = raw_records(connection)
        manifests = batches(connection)

    assert len(rows) == N + 1, "a corrected record must ADD a row, never overwrite one"
    versions = [row for row in rows if row[1] == "C1"]
    assert len(versions) == 2
    assert versions[0][3] != versions[1][3], "two versions of one key share a content_hash"
    # Both the prior and the new version are present, identified by the hash each
    # delivery's own bytes produce.
    assert {row[3] for row in versions} == {
        expected_hashes(BASE_ROWS)["C1"],
        expected_hashes(corrected)["C1"],
    }
    assert {json.loads(row[2])["last_name"] for row in versions} == {"Lovelace", "Byron"}

    assert len(manifests) == 2
    _batch, _run, _source, _path, new, changed, unchanged = manifests[1][:7]
    assert (new, changed, unchanged) == (0, 1, N - 1)


def test_run_stages_counters_and_stdout_manifest(
    initialised_lake: duckdb.DuckDBPyConnection, delivery: Path
) -> None:
    """AC6/AC7: the stage row, its counters, and the two output streams."""
    run_id = str(ULID())

    result = run_er("ingest", "--source", SOURCE, "--path", str(delivery), "--run-id", run_id)

    assert result.returncode == int(ExitCode.SUCCESS), result.stdout + result.stderr

    with connect() as connection:
        runs = query(
            connection,
            f"SELECT run_id, mode, status FROM {SCHEMA_QUALIFIER}.runs",
        )
        stages = query(
            connection,
            f"SELECT stage, seq, status, snapshot_start, snapshot_end, rows_in, rows_out, "
            f"duration_ms, counters, error_class FROM {SCHEMA_QUALIFIER}.run_stages",
        )
        manifests = batches(connection)

    # AC7: a standalone stage invocation is one `runs` row in `stage` mode with one
    # `run_stages` row naming which stage it was (S5).
    assert runs == [(run_id, "stage", "succeeded")]
    assert len(stages) == 1

    (stage, seq, status, start, end, rows_in, rows_out, duration, counters, error) = stages[0]
    assert (stage, seq, status) == ("ingest", 1, "succeeded")
    assert error is None
    # A range, never a count (S4 preamble): a stage may commit many snapshots, and
    # asserting on how many would make a legitimate outcome a failure.
    assert start is not None and end is not None
    assert end >= start
    assert rows_in == N, "S4.1.1: rows_in is the source rows read"
    assert rows_out == N, "S4.1.1: rows_out is the raw_records rows appended"
    assert duration is not None

    # AC6: the payload's key set is exactly the S5.2 union -- no missing name, and no
    # sixth counter beside the five S4.1 counts.
    payload = json.loads(counters)
    assert set(payload) == EXPECTED_COUNTER_KEYS
    assert payload["files"] == 1
    assert payload["ingest_batch_id"] == manifests[0][0]
    assert (payload["new_count"], payload["changed_count"], payload["unchanged_count"]) == (
        N,
        0,
        0,
    )
    assert (payload["tombstone_count"], payload["resurrected_count"]) == (0, 0)
    assert (payload["rows_in"], payload["rows_out"]) == (rows_in, rows_out)

    # AC7: stdout is the S4.0 manifest line and nothing else.
    lines = result.stdout.strip().splitlines()
    assert len(lines) == 1, f"stdout carried more than the manifest line: {lines}"
    assert lines[0] == (
        f"source={SOURCE}, files=1, new={N}, changed=0, unchanged=0, "
        f"tombstoned=0, resurrected=0, ingest_batch_id={manifests[0][0]}"
    )

    # AC7: stderr is exactly one S5.2 record, keyed by this run.
    records = stage_records(result.stderr)
    assert len(records) == 1
    assert records[0]["run_id"] == run_id
    assert records[0]["stage"] == "ingest"
