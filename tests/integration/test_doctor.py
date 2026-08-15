"""T-DOCTOR-1 at its S8.3 node id (DesignDoc.md S2.1, S4.0, S4.0b, S8.3, S9.1).

S8.3 pins this file and `test_doctor_passes`, and S12's M1 exit criterion names the
same thing: "`er doctor` exits 0 with every S2.1 row it owns plus the six T-DOCTOR-1
runtime assertions passing". S9.1 runs the command as the integration job's first step
precisely so a broken substrate fails *before* the suite rather than somewhere inside
it, which is only worth anything if each of the six is individually falsifiable — a
check that cannot be made to fail is a line of output, not an assertion.

So three of the four tests here break something on purpose:

* a `DATA_PATH` pointing at a bucket that does not exist must fail the round-trip row
  and **only** that row (S4.0b calls the round-trip "the backstop that catches a lake
  attached to the wrong place");
* a relation named `__splink__probe` in `lake.main` must fail the `__splink__` row and
  restore exit 0 when it is dropped (M17);
* and the whole command must leave `runs` and `run_stages` untouched and take no
  advisory lock, because ``run_stages.stage`` has no `doctor` value in the S5 enum and
  a doctor that took the writer lock could not run beside the pipeline it diagnoses.

Every invocation is a real subprocess. The check that matters — "does `er doctor` exit
1?" — is a property of the process, and calling :func:`~er.doctor.run_checks` in-process
would assert the table while leaving the command's exit code untested.
"""

from __future__ import annotations

import json
import os
import subprocess
from collections.abc import Iterator, Mapping
from typing import Any

import duckdb
import pytest
from ulid import ULID

from er.doctor import CHECKS, RUNTIME_CHECK_NAMES
from er.lake.catalog import CatalogConnection, drop_metadata_schema, tenant_lock
from er.lake.ducklake import connect
from er.lake.model import SCHEMA_QUALIFIER

#: `configs/test.yaml`'s tenant, which Compose supplies as `ER_CONFIG` (S7.1). A
#: literal, so a wrong value in the document is a failure rather than a tautology.
TENANT = "test"

#: The T-DOCTOR-1 row that a wrong `DATA_PATH` must falsify, and the one a committed
#: Splink intermediate must. Read from the constant rather than respelled, so a rename
#: of either check breaks here instead of silently matching nothing.
ROUND_TRIP_CHECK = RUNTIME_CHECK_NAMES[3]
SPLINK_CHECK = RUNTIME_CHECK_NAMES[5]

#: A bucket `objectstore-init` never creates (S7.1 creates exactly `lake`), so the
#: round-trip below fails on a missing bucket rather than on credentials.
ABSENT_BUCKET = "er-doctor-no-such-bucket"

#: A relation whose name is what S4.0b forbids in the lake: Splink writes its
#: intermediates into `splink_scratch` in the in-memory database, so anything matching
#: `__splink__%` here was committed to DuckLake (M17).
SPLINK_RELATION = "__splink__probe"


def run_er(*args: str, env: Mapping[str, str] | None = None) -> subprocess.CompletedProcess[str]:
    """Invoke the installed `er` console script, in this session's namespace by default."""
    environment = dict(os.environ)
    if env is not None:
        environment.update(env)
    return subprocess.run(
        ["er", *args], capture_output=True, text=True, env=environment, check=False
    )


def check_records(stdout: str) -> list[dict[str, Any]]:
    """The `--json` check objects, in print order."""
    return [json.loads(line) for line in stdout.splitlines() if line.strip()]


def verdicts(stdout: str) -> dict[str, str]:
    """``{check name: verdict}`` for one `er doctor --json` invocation."""
    return {record["check"]: record["verdict"] for record in check_records(stdout)}


def failed(stdout: str) -> list[dict[str, Any]]:
    """Only the failed check objects, so a message can name what actually broke."""
    return [record for record in check_records(stdout) if record["verdict"] == "fail"]


def ledger_counts() -> tuple[int, int]:
    """`(runs, run_stages)` row counts, read on a connection of their own.

    A fresh connection because the rows under test would have been written by another
    process; a cached handle would answer with what this session last saw.
    """
    with connect() as connection:
        return (_count(connection, "runs"), _count(connection, "run_stages"))


def _count(connection: duckdb.DuckDBPyConnection, relation: str, where: str = "") -> int:
    row = connection.execute(
        f"SELECT count(*) FROM {SCHEMA_QUALIFIER}.{relation} {where}"
    ).fetchone()
    return 0 if row is None else int(row[0])


@pytest.fixture
def absent_data_path_env(catalog: CatalogConnection) -> Iterator[Mapping[str, str]]:
    """A namespace whose `DATA_PATH` bucket does not exist, reclaimed afterwards.

    A namespace of its own rather than the session's with one variable overridden:
    `DATA_PATH` is written into the catalog on first attach and is immutable
    thereafter (S4.0b), so attaching the session's metadata schema against a different
    path would either be refused or silently re-point the namespace every later test
    depends on. Only the catalog schema needs reclaiming — the whole point of the
    fixture is that its prefix never existed.
    """
    schema = f"{os.environ['ER_LAKE_METADATA_SCHEMA']}_absent"
    try:
        yield {
            "ER_LAKE_METADATA_SCHEMA": schema,
            "ER_LAKE_DATA_PATH": f"s3://{ABSENT_BUCKET}/{schema}/",
        }
    finally:
        drop_metadata_schema(catalog, schema)


def test_doctor_passes(initialised_lake: duckdb.DuckDBPyConnection) -> None:
    """T-DOCTOR-1: a healthy stack is exactly `len(CHECKS)` passing lines and exit 0."""
    human = run_er("doctor")
    assert human.returncode == 0, human.stdout + human.stderr

    lines = human.stdout.strip().splitlines()
    assert len(lines) == len(CHECKS), f"expected {len(CHECKS)} check lines, got {len(lines)}"

    machine = run_er("doctor", "--json")
    assert machine.returncode == 0, machine.stdout + machine.stderr
    records = check_records(machine.stdout)
    assert [record["check"] for record in records] == [check.name for check in CHECKS]

    # Four columns on every line, and every verdict a pass. The `expected`/`actual`
    # pair is what makes a failure diagnosable from CI output alone (S4.0).
    for record in records:
        assert set(record) == {"check", "expected", "actual", "verdict"}
        assert record["verdict"] == "pass", record
        assert record["actual"] == record["expected"]

    # The six runtime assertions are present, not merely the pin rows: a doctor that
    # asserted S2.1 and nothing else would pass on a substrate that does not work.
    assert set(RUNTIME_CHECK_NAMES) <= {record["check"] for record in records}


def test_bad_data_path_fails_only_the_roundtrip_check(
    initialised_lake: duckdb.DuckDBPyConnection,
    absent_data_path_env: Mapping[str, str],
) -> None:
    """AC4: an unreachable `DATA_PATH` falsifies the round-trip row and nothing else.

    S4.0b's warning is that a `DATA_PATH` which never resolved — the shell-form `$VAR`
    mistake, or a bucket that was never created — attaches without error and fails far
    from its cause. That is only caught if this row is independent of the others, so
    "only" is the assertion.
    """
    result = run_er("doctor", "--json", env=absent_data_path_env)

    assert result.returncode == 1, result.stdout + result.stderr
    assert [record["check"] for record in failed(result.stdout)] == [ROUND_TRIP_CHECK], (
        f"a missing DATA_PATH bucket failed more than the round-trip row: {failed(result.stdout)}"
    )


def test_splink_relation_in_lake_fails_doctor(
    initialised_lake: duckdb.DuckDBPyConnection,
) -> None:
    """AC5: a `__splink__` relation in the lake fails the doctor until it is dropped.

    S4.0b hands Splink the connection with ``output_schema='splink_scratch'`` so its
    intermediates stay in the in-memory database. One that reached DuckLake would
    commit a snapshot per intermediate, so this check is the M17 guard rather than a
    tidiness preference.
    """
    assert run_er("doctor").returncode == 0, "the namespace was not clean before the test"

    initialised_lake.execute(
        f"CREATE TABLE {SCHEMA_QUALIFIER}.{SPLINK_RELATION} AS SELECT 1 AS probe"
    )
    try:
        contaminated = run_er("doctor", "--json")
        assert contaminated.returncode == 1, contaminated.stdout + contaminated.stderr
        assert [record["check"] for record in failed(contaminated.stdout)] == [SPLINK_CHECK]
        assert verdicts(contaminated.stdout)[SPLINK_CHECK] == "fail"
    finally:
        initialised_lake.execute(f"DROP TABLE {SCHEMA_QUALIFIER}.{SPLINK_RELATION}")

    # Dropping it restores exit 0: the check reads the lake every time rather than
    # caching a verdict from the connection it opened first.
    restored = run_er("doctor")
    assert restored.returncode == 0, restored.stdout + restored.stderr


def test_doctor_writes_no_run_rows(initialised_lake: duckdb.DuckDBPyConnection) -> None:
    """AC6: no `runs` row, no `run_stages` row, and no advisory lock.

    Both halves are S5's decision rather than a shortcut. ``run_stages.stage`` has no
    `doctor` value in the S5 enum, so there is no row this command could legally
    write; and S4.7 lists `doctor` among the two commands the writer lock excludes, so
    a doctor run beside a live pipeline must not be refused — and must not refuse one.
    """
    before = ledger_counts()
    run_id = str(ULID())

    result = run_er("doctor", "--run-id", run_id)
    assert result.returncode == 0, result.stdout + result.stderr

    assert ledger_counts() == before, "`er doctor` wrote to the S5.2 ledger"
    with connect() as connection:
        assert _count(connection, "runs", f"WHERE run_id = '{run_id}'") == 0
        assert _count(connection, "run_stages", f"WHERE run_id = '{run_id}'") == 0

    # No lock taken: a second doctor runs to completion while a writer holds the
    # tenant, and exits 0 rather than 3.
    with tenant_lock(TENANT, run_id=str(ULID())):
        concurrent = run_er("doctor")
    assert concurrent.returncode == 0, concurrent.stdout + concurrent.stderr
    assert "writer lock held for tenant" not in concurrent.stderr
