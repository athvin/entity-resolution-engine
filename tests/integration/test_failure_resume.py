"""What a mid-run failure leaves, and what `--resume` does with it (S4.7).

S4.7: "A failure at stage *k* leaves stages `1..k-1` fully committed with
`status='succeeded'` and their snapshot ranges recorded, and stage *k* with
`status='failed'` … and populated `error_class`/`error_detail`. The `runs` row is
`status='failed'`." Then `er run-all --resume <run_id>` restarts from stage *k* under
the original `run_id` and `config_hash`.

The failed run is seeded by driving :class:`~er.obs.runctx.RunContext` in-process, for
the reason ER-023's own integration test gives: while every M1 pipeline stage is a
nothing-to-do stub, a raising stage body is not reachable through the CLI, and a test
that waited for one would be waiting on a later milestone. The *resume* is a real
subprocess, because "restarts the chain" is a claim about the command.

The last test is S1's tenancy statement in executable form. It is here rather than in a
unit test because the claim is about the relations that exist in the lake, and a
registry that agreed with itself would prove nothing about what `er init` created.
"""

from __future__ import annotations

import io
import json
import os
import subprocess
from pathlib import Path
from typing import Any

import duckdb
import pytest
import yaml
from ulid import ULID

from er.config.hashing import config_hash
from er.config.loader import load_config
from er.errors import ErrorClass, StageFailure
from er.lake.ducklake import LAKE_ALIAS, connect
from er.lake.model import REGISTRY, SCHEMA_QUALIFIER
from er.obs.runctx import RunContext, held

TENANT = "test"

#: The four stages `er run-all --skip-ingest` chains, in order (S4.0).
CHAIN_STAGES = ("standardize", "match", "reconcile", "assemble")

#: The stage the seeded run fails at, and the class it fails under. `transient_io` so
#: the assertion is that the raiser's class reached the column, not that the default
#: did: `data` is what an unclassified failure would also have written.
FAILED_STAGE = "match"
FAILURE_CLASS = ErrorClass.TRANSIENT_IO
FAILURE_DETAIL = "s3 returned 503 for the scored-pair write"

#: The one relation S1 permits a `tenant` column on.
TENANT_COLUMN_OWNER = "runs"


def run_er(*args: str) -> subprocess.CompletedProcess[str]:
    """Invoke the installed `er` console script in this session's namespace."""
    return subprocess.run(
        ["er", *args], capture_output=True, text=True, env=dict(os.environ), check=False
    )


def config_path() -> Path:
    """The S6 document Compose supplies, which is what the CLI will hash (S7.1)."""
    return Path(os.environ["ER_CONFIG"])


def query(connection: duckdb.DuckDBPyConnection, sql: str, *parameters: Any) -> list[Any]:
    return connection.execute(sql, list(parameters)).fetchall()


def stage_records(stderr: str) -> list[dict[str, Any]]:
    """The S5.2 records on stderr, in emission order."""
    return [json.loads(line) for line in stderr.splitlines() if line.startswith("{")]


def seed_failed_run(
    connection: duckdb.DuckDBPyConnection, *, run_id: str, on_disk_hash: str
) -> io.StringIO:
    """Leave the ledger a run that failed at :data:`FAILED_STAGE` would leave.

    The provenance columns come from the same document the CLI reads, so a `--resume`
    of this run compares equal hashes — the seeded values are the run's real ones, not
    stand-ins that would make the config-drift guard vacuous.
    """
    document = load_config(config_path())
    stream = io.StringIO()
    context = RunContext(
        run_id=run_id,
        mode="incremental",
        tenant=document.tenant,
        config_hash=on_disk_hash,
        std_version=document.versions.std_version,
        survivorship_version=document.versions.survivorship_version,
        source=held(connection),
        stream=stream,
    )
    with pytest.raises(StageFailure), context as run:
        with run.stage("standardize") as first:
            first.finish(0)
        with run.stage(FAILED_STAGE):
            raise StageFailure(FAILURE_DETAIL, error_class=FAILURE_CLASS)
    return stream


def test_failed_stage_leaves_prefix_committed(
    initialised_lake: duckdb.DuckDBPyConnection,
) -> None:
    """AC7: stages 1..k-1 committed, stage k classified, the run failed."""
    run_id = str(ULID())
    on_disk_hash = config_hash(load_config(config_path()))

    stream = seed_failed_run(initialised_lake, run_id=run_id, on_disk_hash=on_disk_hash)

    stages = query(
        initialised_lake,
        f"SELECT stage, seq, status, snapshot_start, snapshot_end, error_class, error_detail "
        f"FROM {SCHEMA_QUALIFIER}.run_stages WHERE run_id = ? ORDER BY seq",
        run_id,
    )
    assert [(row[0], row[1], row[2]) for row in stages] == [
        ("standardize", 1, "succeeded"),
        (FAILED_STAGE, 2, "failed"),
    ]
    for stage, _seq, _status, start, end, *_rest in stages:
        # A failed stage still committed whatever it committed before it failed, so
        # its range is recorded rather than left NULL — that range is the only recovery
        # tool S4.7 offers, since there is no rollback.
        assert start is not None and end is not None, f"{stage} recorded no snapshot range"
        assert end >= start

    committed, failed = stages
    assert committed[5] is None and committed[6] is None
    assert failed[5] == FAILURE_CLASS.value, "S4.7's class did not reach run_stages"
    assert failed[6] == FAILURE_DETAIL

    assert query(
        initialised_lake, f"SELECT status FROM {SCHEMA_QUALIFIER}.runs WHERE run_id = ?", run_id
    ) == [("failed",)]

    # S5.2: the same record on stderr, carrying both columns.
    record = stage_records(stream.getvalue())[-1]
    assert record["stage"] == FAILED_STAGE
    assert record["status"] == "failed"
    assert record["error_class"] == FAILURE_CLASS.value
    assert record["error_detail"] == FAILURE_DETAIL


def test_resume_restarts_from_failed_stage_without_duplicating_rows(
    initialised_lake: duckdb.DuckDBPyConnection,
) -> None:
    """AC5/AC6: the resume re-executes stage k onwards, and both refusals hold."""
    run_id = str(ULID())
    on_disk_hash = config_hash(load_config(config_path()))
    seed_failed_run(initialised_lake, run_id=run_id, on_disk_hash=on_disk_hash)

    before = dict(
        query(
            initialised_lake,
            f"SELECT stage, started_at FROM {SCHEMA_QUALIFIER}.run_stages WHERE run_id = ?",
            run_id,
        )
    )

    resumed = run_er("run-all", "--mode", "incremental", "--skip-ingest", "--resume", run_id)

    assert resumed.returncode == 0, resumed.stdout + resumed.stderr
    # "restarts the chain from there": stage k runs again in full, and so does every
    # stage after it — none of them ever ran.
    assert [record["stage"] for record in stage_records(resumed.stderr)] == list(
        CHAIN_STAGES[CHAIN_STAGES.index(FAILED_STAGE) :]
    )
    assert {record["run_id"] for record in stage_records(resumed.stderr)} == {run_id}

    with connect() as connection:
        stages = query(
            connection,
            f"SELECT stage, seq, status, started_at FROM {SCHEMA_QUALIFIER}.run_stages "
            f"WHERE run_id = ? ORDER BY seq",
            run_id,
        )
        runs = query(
            connection,
            f"SELECT run_id, config_hash, status, mode FROM {SCHEMA_QUALIFIER}.runs "
            f"WHERE run_id = ?",
            run_id,
        )

    # `(run_id, stage)` still holds exactly one row per stage: the re-executed stage
    # updated its row and kept its `seq` rather than appending a second one (S5.2).
    assert [(row[0], row[1], row[2]) for row in stages] == [
        (stage, seq, "succeeded") for seq, stage in enumerate(CHAIN_STAGES, start=1)
    ]
    # The stages the failed run had already committed were not touched: their
    # `started_at` is when they started, not when the resume ran.
    assert {row[0]: row[3] for row in stages}["standardize"] == before["standardize"]

    assert len(runs) == 1, "the resume minted a second `runs` row"
    assert runs[0] == (run_id, on_disk_hash, "succeeded", "incremental")

    # AC6, first arm: the run now succeeded, so there is nothing left to resume.
    finished = run_er("run-all", "--mode", "incremental", "--skip-ingest", "--resume", run_id)
    assert finished.returncode == 3, finished.stdout + finished.stderr
    assert "already succeeded" in finished.stderr


def test_resume_refuses_a_config_that_moved(
    initialised_lake: duckdb.DuckDBPyConnection, tmp_path: Path
) -> None:
    """AC6, second arm: a document whose hash differs is exit 2, with both hashes named."""
    run_id = str(ULID())
    on_disk_hash = config_hash(load_config(config_path()))
    seed_failed_run(initialised_lake, run_id=run_id, on_disk_hash=on_disk_hash)

    # The repo's own document, edited. `generator.seed` is in the hashed canonical
    # form and changes nothing else about this run, so what the guard sees is a moved
    # `config_hash` and not a second difference it might have been reacting to.
    document = yaml.safe_load(config_path().read_text(encoding="utf-8"))
    document["generator"]["seed"] = int(document["generator"]["seed"]) + 1
    edited = tmp_path / "drifted.yaml"
    edited.write_text(yaml.safe_dump(document, sort_keys=False), encoding="utf-8")
    drifted_hash = config_hash(load_config(edited))
    assert drifted_hash != on_disk_hash, "the edit did not move the config_hash"

    refused = run_er(
        "run-all",
        "--mode",
        "incremental",
        "--skip-ingest",
        "--resume",
        run_id,
        "--config",
        str(edited),
    )

    assert refused.returncode == 2, refused.stdout + refused.stderr
    assert on_disk_hash in refused.stderr
    assert drifted_hash in refused.stderr
    # Refused before any stage ran, so the failed run is exactly as it was.
    assert stage_records(refused.stderr) == []
    with connect() as connection:
        assert query(
            connection,
            f"SELECT status FROM {SCHEMA_QUALIFIER}.run_stages WHERE run_id = ? AND stage = ?",
            run_id,
            FAILED_STAGE,
        ) == [("failed",)]


def test_no_tenant_column_outside_runs(initialised_lake: duckdb.DuckDBPyConnection) -> None:
    """AC8: S1's namespace-only tenancy, asserted on the registry and on the lake.

    S1: "`tenant` names the lake namespace … and is **not** a row-level discriminator.
    No table has a `tenant` column." `runs.tenant` is the single exception S5's DDL
    declares, and it records which namespace a run belonged to rather than partitioning
    anything. A second one would make cross-tenant matching expressible as a predicate,
    which is precisely what S1 removes by construction.
    """
    declared = {name for name, spec in REGISTRY.items() if "tenant" in set(spec.column_names)}
    assert declared == {TENANT_COLUMN_OWNER}

    live = {
        name
        for (name,) in query(
            initialised_lake,
            "SELECT DISTINCT table_name FROM duckdb_columns() "
            "WHERE database_name = ? AND column_name = 'tenant'",
            LAKE_ALIAS,
        )
    }
    assert live == {TENANT_COLUMN_OWNER}
