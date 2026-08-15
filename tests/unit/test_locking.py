"""The S4.7 taxonomy and the S4.0b lock's contract, without a catalog (S8.4).

Three properties are asserted here and nowhere else:

* the seven-row error table, both of its columns, and what an unclassified failure
  is recorded as;
* the lock key, which two processes MUST derive identically from one `tenant` —
  a key that varied by process would let both writers believe they held the lock;
* release in a ``finally``, which is what stops a crashed writer from wedging the
  tenant until someone unlocks it by hand (S4.7 AC3).

The lock's *exclusion* needs a real Postgres session and lives in
``tests/integration/test_concurrency.py``. What is unit-testable is the statement
sequence, and it is tested with a recording connection rather than a live one so the
release path is asserted on the failure branch, which a live test can only reach by
raising inside someone else's block.
"""

from __future__ import annotations

from collections.abc import Iterator
from contextlib import contextmanager
from pathlib import Path
from typing import Any

import pytest
from typer.testing import CliRunner

from er.cli import COMMANDS, MUTATING_COMMANDS, READ_ONLY_COMMANDS, app
from er.errors import (
    ERROR_CLASS_TABLE,
    ERROR_CLASS_TO_EXIT,
    RETRYABLE,
    ConfigError,
    ErrorClass,
    ExitCode,
    NothingToDo,
    PreconditionFailure,
    StageFailure,
    classify,
)
from er.lake import catalog
from er.lake.catalog import (
    LOCK_APPLICATION_PREFIX,
    LOCK_HELD_MESSAGE,
    UNKNOWN_RUN,
    advisory_lock_key,
    tenant_lock,
)

REPO_ROOT = Path(__file__).resolve().parents[2]
TEST_CONFIG = REPO_ROOT / "configs" / "test.yaml"

#: S4.7's table, transcribed here from the spec rather than imported from the code,
#: so this test compares two independent readings of it. `(exit, retryable)`.
S4_7_TABLE: dict[str, tuple[int, bool]] = {
    "transient_io": (1, True),
    "lock_conflict": (3, True),
    "precondition": (3, False),
    "config": (2, False),
    "contradiction": (1, False),
    "non_convergence": (1, False),
    "data": (1, False),
}

#: The lock key for `configs/test.yaml`'s tenant, pinned as a literal. It is a value
#: two *processes* must agree on, so a test that recomputed it with the same function
#: would agree with any derivation, including a randomised one.
TEST_TENANT_KEY = -7589344150037305645
OTHER_TENANT_KEY = -3003674107523525713


class RecordingConnection:
    """A catalog connection that records statements and answers the lock probe."""

    def __init__(self, *, acquired: bool = True, holder: str | None = None) -> None:
        self.acquired = acquired
        self.holder = holder
        self.statements: list[tuple[str, tuple[Any, ...]]] = []
        self.closed = False

    def execute(self, statement: str, parameters: tuple[Any, ...] = ()) -> RecordingConnection:
        self.statements.append((statement, parameters))
        return self

    def fetchone(self) -> tuple[Any, ...] | None:
        statement, _parameters = self.statements[-1]
        if "pg_try_advisory_lock" in statement:
            return (self.acquired,)
        if "pg_locks" in statement:
            return None if self.holder is None else (self.holder,)
        return (None,)

    def close(self) -> None:
        self.closed = True


@contextmanager
def _patched(
    monkeypatch: pytest.MonkeyPatch, connection: RecordingConnection
) -> Iterator[RecordingConnection]:
    """Make :func:`~er.lake.catalog.catalog_connect` yield ``connection``."""

    @contextmanager
    def fake_connect(dsn: str | None = None) -> Iterator[RecordingConnection]:
        try:
            yield connection
        finally:
            connection.close()

    monkeypatch.setattr(catalog, "catalog_connect", fake_connect)
    yield connection


class Boom(Exception):
    """Raised inside a held lock to prove the release runs on the failure path."""


def test_error_class_table_matches_s4_7() -> None:
    """AC1: the table is S4.7's, both columns, and nothing is missing from it."""
    assert {str(error_class) for error_class in ERROR_CLASS_TABLE} == set(S4_7_TABLE)
    assert {str(error_class) for error_class in ErrorClass} == set(S4_7_TABLE)

    for name, (exit_code, retryable) in S4_7_TABLE.items():
        error_class = ErrorClass(name)
        row = ERROR_CLASS_TABLE[error_class]
        assert row.error_class is error_class
        assert int(row.exit_code) == exit_code, f"{name} does not exit {exit_code}"
        assert row.retryable is retryable, f"{name} retryability disagrees with S4.7"
        # The two ER-014 views are the table read down its columns, not a second copy.
        assert ERROR_CLASS_TO_EXIT[error_class] is row.exit_code
        assert RETRYABLE[error_class] is row.retryable

    # AC1's second half: an exception carrying no class is `data`, and `data` exits 1.
    for unclassified in (ValueError("nope"), RuntimeError("nope"), StageFailure("nope")):
        assert classify(unclassified) is ErrorClass.DATA
        assert int(ERROR_CLASS_TABLE[classify(unclassified)].exit_code) == 1

    # A classified failure is honoured, whichever hierarchy it came from.
    assert classify(PreconditionFailure("held", error_class=ErrorClass.LOCK_CONFLICT)) is (
        ErrorClass.LOCK_CONFLICT
    )
    assert classify(PreconditionFailure("no active model")) is ErrorClass.PRECONDITION
    assert classify(ConfigError("bad yaml")) is ErrorClass.CONFIG
    # `10` is outside the taxonomy: S4.0 makes it a successful no-op, so `NothingToDo`
    # carries no class and is never a row in the table.
    assert NothingToDo("empty delivery").error_class is None


def test_lock_key_derives_from_tenant() -> None:
    """AC2/AC4: one tenant, one key, stable across processes; two tenants, two keys."""
    assert advisory_lock_key("test") == TEST_TENANT_KEY
    assert advisory_lock_key("other") == OTHER_TENANT_KEY
    assert advisory_lock_key("test") != advisory_lock_key("other")
    # `pg_advisory_lock` takes a signed bigint and refuses anything wider.
    assert -(2**63) <= advisory_lock_key("test") < 2**63


def test_lock_released_in_finally_on_exception(monkeypatch: pytest.MonkeyPatch) -> None:
    """AC3: the block raising does not skip the unlock, and the session still closes."""
    connection = RecordingConnection()
    with _patched(monkeypatch, connection):
        with pytest.raises(Boom), tenant_lock("test", run_id="01JRESUMEDRUNIDXXXXXXXXXXX"):
            raise Boom

    statements = [statement for statement, _parameters in connection.statements]
    assert any("pg_try_advisory_lock" in statement for statement in statements)
    assert any("pg_advisory_unlock" in statement for statement in statements), (
        "S4.7: the lock must be released in a finally, including on failure"
    )
    assert statements.index("SELECT pg_advisory_unlock(%s)") > statements.index(
        "SELECT pg_try_advisory_lock(%s)"
    )
    # Closing the session is the belt to the unlock's braces: a session-scoped advisory
    # lock dies with its session, which is what releases the tenant when a writer is
    # killed outright and never reaches its own `finally`.
    assert connection.closed

    # The run id is advertised on the session, not written anywhere that could outlive
    # it, so the next contender can name this writer.
    advertised = [
        parameters for statement, parameters in connection.statements if "set_config" in statement
    ]
    assert advertised == [(f"{LOCK_APPLICATION_PREFIX}01JRESUMEDRUNIDXXXXXXXXXXX",)]


def test_refusal_carries_the_s4_7_message_and_class(monkeypatch: pytest.MonkeyPatch) -> None:
    """AC2: a held lock is exit 3, class `lock_conflict`, and names tenant and run."""
    holder = "01JHOLDINGRUNIDXXXXXXXXXXX"
    connection = RecordingConnection(acquired=False, holder=f"{LOCK_APPLICATION_PREFIX}{holder}")
    with _patched(monkeypatch, connection), pytest.raises(PreconditionFailure) as refusal:
        with tenant_lock("test", run_id="01JSECONDRUNIDXXXXXXXXXXXX"):
            pytest.fail("the lock was granted while another writer held it")

    assert str(refusal.value) == LOCK_HELD_MESSAGE.format(tenant="test", run_id=holder)
    assert str(refusal.value) == f"writer lock held for tenant test by run {holder}"
    assert refusal.value.error_class is ErrorClass.LOCK_CONFLICT
    assert refusal.value.code == int(ExitCode.PRECONDITION)
    assert refusal.value.retryable is True, "S4.7 makes lock_conflict retryable"
    # A holder that advertised nothing still yields S4.7's shape, not a broken message.
    assert not any("pg_advisory_unlock" in statement for statement, _ in connection.statements), (
        "a refused writer must not unlock the holder's lock"
    )


def test_refusal_without_an_advertised_holder_is_still_well_formed(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    """The `<run_id>` degrades to a sentinel; the exit code and class do not."""
    connection = RecordingConnection(acquired=False, holder=None)
    with _patched(monkeypatch, connection), pytest.raises(PreconditionFailure) as refusal:
        with tenant_lock("test"):
            pytest.fail("the lock was granted while another writer held it")

    assert str(refusal.value) == LOCK_HELD_MESSAGE.format(tenant="test", run_id=UNKNOWN_RUN)
    assert refusal.value.error_class is ErrorClass.LOCK_CONFLICT


def test_every_s4_0_command_is_locked_or_declared_read_only() -> None:
    """AC4: S4.7's mutating list is complete over S4.0's command table.

    `review` is the one row of S4.0 that is two commands: `resolve` writes an
    assertion row and `list` reads. Both spellings are accounted for, so a future
    command added to :data:`~er.cli.COMMANDS` and to neither set here fails this
    rather than silently defaulting to unlocked.
    """
    assert MUTATING_COMMANDS.isdisjoint(READ_ONLY_COMMANDS)
    covered = MUTATING_COMMANDS | READ_ONLY_COMMANDS
    assert covered == set(COMMANDS) - {"review"} | {"review list", "review resolve"}
    assert "doctor" in READ_ONLY_COMMANDS
    assert "review list" in READ_ONLY_COMMANDS


def test_mutating_command_runs_unlocked_when_no_catalog_is_configured(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    """A process with no `$ER_CATALOG_DSN` runs unlocked instead of failing.

    This is what lets the S8.4 unit layer drive the whole command tree on a bare
    runner: there is no catalog to lock, and therefore no second writer that could
    exist to contend with it. It is the same judgement `RunContext` makes about
    persistence, and it is asserted rather than assumed because the alternative —
    every mutating command exiting 2 without services — would be silent until CI ran.
    """
    monkeypatch.delenv("ER_CATALOG_DSN", raising=False)

    result = CliRunner().invoke(app, ["standardize", "--config", str(TEST_CONFIG)])

    assert "standardize" in MUTATING_COMMANDS
    assert result.exit_code == int(ExitCode.NOTHING_TO_DO), result.stderr
