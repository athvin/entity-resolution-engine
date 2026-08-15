"""Every mutating command takes the writer lock (DesignDoc.md S4.0b, S4.7).

S4.7 gives the lock a list, not a policy: `init`, `ingest`, `standardize`, `train`,
`match`, `reconcile`, `assemble`, `run-all`, `correct`, `assert`, `review resolve`,
`lake maintain` and `lake reset` take it; `doctor` and `review list` do not. A list is
only enforceable command by command — a command that quietly skipped it would be
invisible in any aggregate — so this asserts each row of it as its own subprocess.

The second test is S4.7's release guarantee read from the outside: a run that failed
is followed by a run that acquires, with no manual unlock step. That is the property
the ``finally`` in :func:`~er.lake.catalog.tenant_lock` exists for, and the only way to
see it is to fail a real command and then run another one.
"""

from __future__ import annotations

import os
import subprocess

import duckdb
import pytest
from ulid import ULID

from er.cli import MUTATING_COMMANDS, READ_ONLY_COMMANDS
from er.lake.catalog import tenant_lock, try_advisory_lock
from er.lake.ducklake import connect
from er.lake.model import SCHEMA_QUALIFIER

TENANT = "test"

#: Every command of S4.7's mutating list, with the flags S4.0 makes required, as the
#: argv a user would type. The key is the command path, so the parametrisation reads
#: as the spec's list and :data:`~er.cli.MUTATING_COMMANDS` can be checked against it.
MUTATING_INVOCATIONS: dict[str, tuple[str, ...]] = {
    "init": ("init",),
    "ingest": ("ingest", "--source", "crm", "--path", "/app/fixtures/static/base_10/base"),
    "standardize": ("standardize",),
    "train": ("train",),
    "match": ("match", "--mode", "full"),
    "reconcile": ("reconcile",),
    "assemble": ("assemble",),
    "run-all": ("run-all", "--mode", "incremental", "--skip-ingest"),
    "correct": ("correct",),
    "assert": ("assert", "add", "--a", "crm:1", "--b", "crm:2", "--kind", "always", "--by", "t"),
    "review resolve": ("review", "resolve", "--review-id", "r1", "--as", "match", "--by", "t"),
    "lake maintain": ("lake", "maintain"),
    "lake reset": ("lake", "reset", "--confirm-tenant", TENANT),
}

#: The two S4.7 excludes, with the flags S4.0 gives them.
READ_ONLY_INVOCATIONS: dict[str, tuple[str, ...]] = {
    "doctor": ("doctor",),
    "review list": ("review", "list"),
}

#: S4.0's exit code for "advisory lock not acquired".
LOCK_CONFLICT_EXIT = 3


def run_er(*args: str) -> subprocess.CompletedProcess[str]:
    """Invoke the installed `er` console script in this session's namespace."""
    return subprocess.run(
        ["er", *args], capture_output=True, text=True, env=dict(os.environ), check=False
    )


def test_the_invocation_table_covers_s4_7s_list() -> None:
    """The parametrisation below is S4.7's list itself, not a sample of it."""
    assert set(MUTATING_INVOCATIONS) == MUTATING_COMMANDS
    assert set(READ_ONLY_INVOCATIONS) == READ_ONLY_COMMANDS


@pytest.mark.parametrize("command", sorted(MUTATING_INVOCATIONS))
def test_mutating_commands_require_the_lock(
    command: str, initialised_lake: duckdb.DuckDBPyConnection
) -> None:
    """AC4: with the lock held by another session, every mutating command exits 3."""
    with tenant_lock(TENANT, run_id=str(ULID())):
        result = run_er(*MUTATING_INVOCATIONS[command])

    assert result.returncode == LOCK_CONFLICT_EXIT, (
        f"`er {command}` did not refuse a contended namespace: "
        f"exit {result.returncode}\n{result.stdout}{result.stderr}"
    )
    assert "writer lock held for tenant" in result.stderr


@pytest.mark.parametrize("command", sorted(READ_ONLY_INVOCATIONS))
def test_read_only_commands_do_not_take_the_lock(
    command: str, initialised_lake: duckdb.DuckDBPyConnection
) -> None:
    """AC4: `doctor` and `review list` run while another writer holds the lock.

    They are asserted NOT to be refused rather than asserted to exit 0. Both are still
    M1 stubs — S4.0 gives `er doctor` exit `1` for "any check fails", and an
    unimplemented stage is exactly that — so exit 0 is a claim about T-DOCTOR-1's
    implementation and about the `review_queue` reader, neither of which is this
    ticket's. What is this ticket's, and what is asserted, is that neither command is
    refused by a lock it does not take: the exit is not 3, and the reason is the stub's
    own, not a contended namespace.
    """
    with tenant_lock(TENANT, run_id=str(ULID())):
        result = run_er(*READ_ONLY_INVOCATIONS[command])

    assert result.returncode != LOCK_CONFLICT_EXIT, (
        f"`er {command}` was refused by a lock S4.7 says it does not take"
    )
    assert "writer lock held for tenant" not in result.stderr


def test_lock_is_reacquirable_after_failure(
    initialised_lake: duckdb.DuckDBPyConnection,
) -> None:
    """AC3: a run that exits non-zero releases the lock, with no manual unlock step."""
    assert try_advisory_lock(TENANT) is True, "the tenant was already locked"

    failed_run = str(ULID())
    failed = run_er("train", "--run-id", failed_run)

    # `er train` is a mutating command whose M1 stage is unimplemented, so it takes the
    # lock, fails inside it, and is the shortest real path to "a run that exited
    # non-zero mid-stage" while every pipeline stage is still a no-op stub.
    assert failed.returncode != 0, failed.stdout + failed.stderr
    with connect() as connection:
        assert connection.execute(
            f"SELECT status FROM {SCHEMA_QUALIFIER}.runs WHERE run_id = ?", [failed_run]
        ).fetchall() == [("failed",)]

    assert try_advisory_lock(TENANT) is True, "the failed run left the tenant locked"

    after = run_er("run-all", "--mode", "incremental", "--skip-ingest")
    assert after.returncode == 0, after.stdout + after.stderr
