"""`runs`, `run_stages` and the touched set, against the Compose substrate (S5.2).

M2 is that `run_id` was referenced by six relations and had no referent. Every
assertion here is therefore about what survived in the lake, not about what a
function returned.

Two shapes are used deliberately. The chain assertions spawn `er` as a subprocess,
exactly as Compose runs it (S7.1), because "every stage of `er run-all` writes its
row" is a claim about the command and not about :class:`~er.obs.runctx.RunContext`.
The failure and touched-set assertions drive the context in-process against the
S8.1 fixture's connection, because a raising stage body and a re-execution of one
are not reachable through a CLI whose M1 stages are no-op stubs.

Snapshot **counts** are asserted nowhere and may not be: a stage commits a range
(S4 preamble), and a no-op run may legitimately commit an empty one.
"""

from __future__ import annotations

import io
import json
import os
import subprocess
from typing import Any

import duckdb
import pytest
from ulid import ULID

from er.errors import ErrorClass, StageFailure
from er.lake.ducklake import connect
from er.lake.model import PROMOTED_COUNTERS, SCHEMA_QUALIFIER
from er.obs.counters import DECLARED_COUNTERS
from er.obs.logging import STAGE_RECORD_KEYS
from er.obs.runctx import RunContext, held
from er.obs.touched import (
    REBUILD,
    RETIRE,
    InvalidDispositionError,
    read_touched_entities,
    write_touched_entities,
)

#: The four stages `er run-all --skip-ingest` chains, in order (S4.0). The M1 exit
#: criterion names exactly these four `run_stages` rows.
CHAIN_STAGES = ("standardize", "match", "reconcile", "assemble")

# `configs/test.yaml`'s tenant, which Compose supplies as `ER_CONFIG` (S7.1). A
# literal, so a wrong value in the document is a failure rather than a tautology.
TENANT = "test"

# Provenance for the in-process runs. `runs` declares all four NOT NULL, and what is
# under test here is the row's lifecycle, not where a real CLI reads them from.
CONFIG_HASH = "a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f90"
STD_VERSION = "v1"
SURVIVORSHIP_VERSION = "v1"


def run_er(*args: str) -> subprocess.CompletedProcess[str]:
    """Invoke the installed `er` console script in this session's namespace."""
    return subprocess.run(
        ["er", *args], capture_output=True, text=True, env=dict(os.environ), check=False
    )


def query(connection: duckdb.DuckDBPyConnection, sql: str, *parameters: Any) -> list[Any]:
    return connection.execute(sql, list(parameters)).fetchall()


def stage_records(stderr: str) -> list[dict[str, Any]]:
    """The S5.2 records on stderr, in emission order."""
    return [json.loads(line) for line in stderr.splitlines() if line.startswith("{")]


def run_context(run_id: str, *, connection: duckdb.DuckDBPyConnection, mode: str) -> RunContext:
    """A context persisting through the fixture's own connection (S8.1)."""
    return RunContext(
        run_id=run_id,
        mode=mode,
        tenant=TENANT,
        config_hash=CONFIG_HASH,
        std_version=STD_VERSION,
        survivorship_version=SURVIVORSHIP_VERSION,
        source=held(connection),
        # The record is what the CLI puts on stderr; captured here so a passing test
        # is silent, and still asserted on -- it is emitted for every stage either way.
        stream=io.StringIO(),
    )


def test_run_all_writes_one_run_and_four_stage_rows(
    initialised_lake: duckdb.DuckDBPyConnection,
) -> None:
    """AC1/AC2/AC5: one `runs` row, four `run_stages` rows, one stderr line each."""
    result = run_er("run-all", "--mode", "incremental", "--skip-ingest")

    assert result.returncode == 0, result.stdout + result.stderr

    # Read on a connection of its own: the rows were committed by another process,
    # so what is asserted is what reached the catalog, not what a cached handle holds.
    with connect() as connection:
        runs = query(
            connection,
            f"SELECT run_id, tenant, mode, status, config_hash, code_version, "
            f"std_version, survivorship_version, started_at, ended_at "
            f"FROM {SCHEMA_QUALIFIER}.runs",
        )
        stages = query(
            connection,
            f"SELECT stage, seq, status, snapshot_start, snapshot_end, started_at, "
            f"ended_at, counters, duration_ms, error_class "
            f"FROM {SCHEMA_QUALIFIER}.run_stages ORDER BY seq",
        )

    assert len(runs) == 1, "S5.2: one `runs` row per CLI invocation"
    (run_id, tenant, mode, status, config_hash, code_version, *_rest) = runs[0]
    assert status == "succeeded"
    assert mode == "incremental"
    assert tenant == TENANT
    assert config_hash
    assert code_version
    assert runs[0][-1] is not None, "the run was never stamped with an `ended_at`"

    assert [row[0] for row in stages] == list(CHAIN_STAGES)
    assert [row[1] for row in stages] == [1, 2, 3, 4], "S5.2: `seq` is dense and 1-based"
    for stage, _seq, stage_status, start, end, started, ended, counters, duration, error in stages:
        assert stage_status == "succeeded", f"{stage} did not succeed"
        assert start is not None and end is not None, f"{stage} recorded no snapshot range"
        assert end >= start, f"{stage} recorded a range that runs backwards"
        assert started is not None and ended is not None
        assert error is None
        assert duration is not None and duration >= 0
        # AC5, the S5.2 completeness rule: the payload is the union of the names S4
        # declares for the stage and every promoted counter the stage wrote.
        payload = json.loads(counters)
        written = {name for name in PROMOTED_COUNTERS if payload.get(name) is not None}
        assert set(payload) >= set(DECLARED_COUNTERS[stage]) | written
        assert "duration_ms" in payload

    # AC2: exactly one record per stage on stderr, keyed by this run's `run_id`.
    records = stage_records(result.stderr)
    assert len(records) == len(CHAIN_STAGES)
    assert {record["run_id"] for record in records} == {run_id}
    for record in records:
        assert tuple(record) == STAGE_RECORD_KEYS


def test_stage_records_snapshot_range(initialised_lake: duckdb.DuckDBPyConnection) -> None:
    """AC1: every row carries a range, and the run's span contains every stage's."""
    result = run_er("run-all", "--mode", "full", "--skip-ingest")

    assert result.returncode == 0, result.stdout + result.stderr

    with connect() as connection:
        run = query(
            connection,
            f"SELECT snapshot_start, snapshot_end FROM {SCHEMA_QUALIFIER}.runs",
        )
        stages = query(
            connection,
            f"SELECT stage, snapshot_start, snapshot_end FROM {SCHEMA_QUALIFIER}.run_stages "
            f"ORDER BY seq",
        )

    assert len(run) == 1
    run_start, run_end = run[0]
    assert run_start is not None and run_end is not None
    assert run_end >= run_start
    assert len(stages) == len(CHAIN_STAGES)
    for stage, start, end in stages:
        # A range, never a count: a no-op stage commits an empty one, and asserting
        # `end > start` anywhere would make that legitimate outcome a failure.
        assert end >= start, f"{stage}: snapshot_end < snapshot_start"
        assert run_start <= start, f"{stage} began before the run's snapshot_start"
        assert run_end >= end, f"{stage} ended after the run's snapshot_end"


def test_failed_stage_marks_run_failed(initialised_lake: duckdb.DuckDBPyConnection) -> None:
    """AC6: the failed stage, its predecessors, the run, and the re-execution."""
    run_id = str(ULID())
    context = run_context(run_id, connection=initialised_lake, mode="incremental")

    with pytest.raises(StageFailure, match="match blew up"), context as run:
        with run.stage("standardize") as first:
            first.finish(0)
        with run.stage("match"):
            raise StageFailure("match blew up")

    stages = query(
        initialised_lake,
        f"SELECT stage, seq, status, error_class, error_detail, ended_at, snapshot_start, "
        f"snapshot_end FROM {SCHEMA_QUALIFIER}.run_stages WHERE run_id = ? ORDER BY seq",
        run_id,
    )
    assert [(row[0], row[1], row[2]) for row in stages] == [
        ("standardize", 1, "succeeded"),
        ("match", 2, "failed"),
    ]
    _stage, _seq, _status, error_class, error_detail, ended_at, start, end = stages[1]
    assert error_class == ErrorClass.DATA.value, "S4.7 leaves no failed stage unclassified"
    assert error_detail == "match blew up"
    assert ended_at is not None
    # A failed stage still committed whatever it committed before it failed (S4.7),
    # so its range is recorded rather than left NULL.
    assert start is not None and end is not None and end >= start
    assert query(
        initialised_lake,
        f"SELECT status FROM {SCHEMA_QUALIFIER}.runs WHERE run_id = ?",
        run_id,
    ) == [("failed",)]

    # Re-executing the failed stage updates its row and keeps its `seq` (S5.2): a
    # second row would break the `(run_id, stage)` key the snapshot range hangs on.
    retry = run_context(run_id, connection=initialised_lake, mode="incremental")
    with retry as run:
        with run.stage("match") as second:
            second.finish(0)

    assert query(
        initialised_lake,
        f"SELECT seq, status, error_class FROM {SCHEMA_QUALIFIER}.run_stages "
        f"WHERE run_id = ? AND stage = 'match'",
        run_id,
    ) == [(2, "succeeded", None)]
    assert query(
        initialised_lake,
        f"SELECT count(*) FROM {SCHEMA_QUALIFIER}.run_stages WHERE run_id = ?",
        run_id,
    ) == [(2,)]
    assert query(
        initialised_lake,
        f"SELECT count(*), max(status) FROM {SCHEMA_QUALIFIER}.runs WHERE run_id = ?",
        run_id,
    ) == [(1, "succeeded")]

    emitted = context.stream
    assert isinstance(emitted, io.StringIO)
    assert len(stage_records(emitted.getvalue())) == 2, "a failed stage still emits its line"


def test_touched_entities_roundtrip_is_idempotent(
    initialised_lake: duckdb.DuckDBPyConnection,
) -> None:
    """AC7: one row per `(run_id, entity_id)`, filtered by run, validated first."""
    run_id = str(ULID())
    other_run = str(ULID())

    assert write_touched_entities(initialised_lake, run_id, [("E1", REBUILD)]) == 1
    assert write_touched_entities(initialised_lake, run_id, [("E1", RETIRE)]) == 1

    assert query(
        initialised_lake,
        f"SELECT count(*) FROM {SCHEMA_QUALIFIER}.er_touched_entities "
        f"WHERE run_id = ? AND entity_id = 'E1'",
        run_id,
    ) == [(1,)], "a second write of the same (run_id, entity_id) added a row"

    write_touched_entities(initialised_lake, other_run, [("E2", REBUILD)])
    mine = read_touched_entities(initialised_lake, run_id)
    assert [entity.entity_id for entity in mine] == ["E1"]
    assert mine[0].disposition == RETIRE
    assert read_touched_entities(initialised_lake, run_id, disposition=REBUILD) == ()
    theirs = read_touched_entities(initialised_lake, other_run)
    assert [entity.entity_id for entity in theirs] == ["E2"]

    # Rejected before ANY write: the valid pair in the same batch must not land, or
    # the caller is left with a half-applied touched set the marts would then join.
    with pytest.raises(InvalidDispositionError, match="banish"):
        write_touched_entities(initialised_lake, run_id, [("E9", REBUILD), ("E3", "banish")])
    after = read_touched_entities(initialised_lake, run_id)
    assert [entity.entity_id for entity in after] == ["E1"]
    with pytest.raises(InvalidDispositionError):
        read_touched_entities(initialised_lake, run_id, disposition="banish")
