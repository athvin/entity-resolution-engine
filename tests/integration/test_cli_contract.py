"""T-CONC-1: single-writer enforcement at its S8.3 node id (DesignDoc.md S4.0b, S4.7).

S8.3 pins this file and this test name, and S12's M1 exit criterion names T-CONC-1
directly: the milestone cannot be signed off without "a second concurrent `er run-all`
exits 3". S13 says why it is an exit criterion rather than a nice-to-have — two writers
against one `tenant` "would interleave snapshot ranges, duplicate events, and race
`MERGE INTO` on `entity_membership`", and concurrency here is an explicit non-guarantee
rather than an untested hope.

The overlap is created by holding the lock for the length of the second invocation
rather than by racing two subprocesses. That is not a weaker test: the first writer's
whole contribution to T-CONC-1 is that it holds the tenant lock, it holds it with the
same :func:`~er.lake.catalog.tenant_lock` an `er run-all` process uses, and the second
invocation is a real `er run-all` that has no way to tell the difference. Racing two
subprocesses would assert the same thing only when the scheduler happened to overlap
them, and would silently assert nothing when it did not.

ER-034 adds T-CFG-1 to this file.
"""

from __future__ import annotations

import json
import os
import subprocess
from typing import Any

import duckdb
from ulid import ULID

from er.lake.catalog import LOCK_HELD_MESSAGE, tenant_lock, try_advisory_lock
from er.lake.ducklake import connect
from er.lake.model import SCHEMA_QUALIFIER

# `configs/test.yaml`'s tenant, which Compose supplies as `ER_CONFIG` (S7.1). A
# literal, so a wrong value in the document is a failure rather than a tautology.
TENANT = "test"


def run_er(*args: str) -> subprocess.CompletedProcess[str]:
    """Invoke the installed `er` console script in this session's namespace."""
    return subprocess.run(
        ["er", *args], capture_output=True, text=True, env=dict(os.environ), check=False
    )


def ledger_counts() -> tuple[int, int]:
    """`(runs, run_stages)` row counts, read on a connection of their own.

    A fresh connection because the rows under test would have been written by another
    process; a cached handle would answer with what this session last saw.
    """
    with connect() as connection:
        return (
            _count(connection, "runs"),
            _count(connection, "run_stages"),
        )


def _count(connection: duckdb.DuckDBPyConnection, relation: str) -> int:
    row = connection.execute(f"SELECT count(*) FROM {SCHEMA_QUALIFIER}.{relation}").fetchone()
    return 0 if row is None else int(row[0])


def stage_records(stderr: str) -> list[dict[str, Any]]:
    """The S5.2 records on stderr, in emission order."""
    return [json.loads(line) for line in stderr.splitlines() if line.startswith("{")]


def test_second_concurrent_run_exits_3(initialised_lake: duckdb.DuckDBPyConnection) -> None:
    """T-CONC-1/AC2: the second writer exits 3, names the first, and writes nothing."""
    first_run_id = str(ULID())
    before = ledger_counts()

    with tenant_lock(TENANT, run_id=first_run_id):
        second = run_er("run-all", "--mode", "incremental", "--skip-ingest")

    assert second.returncode == 3, second.stdout + second.stderr
    # S4.7's literal message, rendered from its one definition, naming the tenant that
    # is contended and the run that holds it.
    assert LOCK_HELD_MESSAGE.format(tenant=TENANT, run_id=first_run_id) in second.stderr
    assert f"writer lock held for tenant {TENANT} by run {first_run_id}" in second.stderr

    # "without writing a `runs` row" (S8.3). Not one row, and not one stage row either:
    # the refusal happens before the RunContext is entered, so there is no ledger entry
    # for a run that never ran and no stage record claiming one began.
    assert ledger_counts() == before
    assert stage_records(second.stderr) == []
    assert second.stdout == "", "a refused writer emitted command output"


def test_the_refusal_leaves_the_tenant_unlocked(
    initialised_lake: duckdb.DuckDBPyConnection,
) -> None:
    """AC3: refusing does not wedge the namespace, and the writer that held it can go.

    The failure this guards against is a contender that takes the lock, discovers it
    is contended, and unlocks — releasing the *holder's* lock, because `pg_advisory_unlock`
    is keyed on the value rather than on the acquisition.
    """
    with tenant_lock(TENANT, run_id=str(ULID())):
        refused = run_er("run-all", "--mode", "incremental", "--skip-ingest")
        assert refused.returncode == 3, refused.stdout + refused.stderr
        assert try_advisory_lock(TENANT) is False, "the refused writer released the holder's lock"

    assert try_advisory_lock(TENANT) is True
    after = run_er("run-all", "--mode", "incremental", "--skip-ingest")
    assert after.returncode == 0, after.stdout + after.stderr
