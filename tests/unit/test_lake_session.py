from collections.abc import Iterator
from contextlib import contextmanager

import duckdb
import pytest

from er.lake import ducklake


@pytest.fixture
def connections(monkeypatch: pytest.MonkeyPatch) -> list[duckdb.DuckDBPyConnection]:
    opened = []

    @contextmanager
    def factory() -> Iterator[duckdb.DuckDBPyConnection]:
        connection = duckdb.connect()
        connection.execute("CREATE SCHEMA splink_scratch")
        opened.append(connection)
        try:
            yield connection
        finally:
            connection.close()

    monkeypatch.setattr(ducklake, "_fresh_connection", factory)
    return opened


def test_session_reuses_native_connection_and_closes_before_dbt(connections: list) -> None:
    with ducklake.invocation_session():
        with ducklake.connect() as first:
            first.execute("CREATE TABLE splink_scratch.old_model AS SELECT 1")
            first.execute("SET schema='splink_scratch'")
        with ducklake.connect() as again:
            assert again is first
        ducklake.clear_session_scratch()
        with ducklake.connect() as clean:
            assert clean.execute("SELECT current_schema()").fetchone() == ("main",)
            assert clean.execute("SELECT count(*) FROM duckdb_tables()").fetchone() == (0,)
        ducklake.suspend_session()
        with pytest.raises(duckdb.ConnectionException):
            first.execute("SELECT 1")
        with ducklake.connect() as reopened:
            assert reopened is not first
    assert len(connections) == 2
    with pytest.raises(duckdb.ConnectionException):
        reopened.execute("SELECT 1")


def test_dbt_cannot_start_with_an_active_borrow(connections: list) -> None:
    with ducklake.invocation_session(), ducklake.connect():
        with pytest.raises(RuntimeError, match="borrowed"):
            ducklake.suspend_session()


def test_failed_transaction_is_discarded_before_failure_bookkeeping(connections: list) -> None:
    with ducklake.invocation_session():
        with pytest.raises(duckdb.Error):
            with ducklake.connect() as failed:
                failed.execute("BEGIN")
                failed.execute("SELECT * FROM missing_table")
        with ducklake.connect() as recorder:
            assert recorder is not failed
            assert recorder.execute("SELECT 42").fetchone() == (42,)
    assert len(connections) == 2


def test_no_session_leaks_into_next_invocation(connections: list) -> None:
    for _ in range(2):
        with ducklake.invocation_session(), ducklake.connect():
            pass
    with ducklake.connect():
        pass
    assert len(connections) == 3
