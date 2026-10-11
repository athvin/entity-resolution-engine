"""``queue.stored_result``: the second recovery channel for the terminal record.

Pure shape tests over a fake connection — the write side is exercised by
``test_runner_execute`` and the integration tier; this pins what the reader
accepts (a dict carrying ``exit_code``) and what it rejects.
"""

from __future__ import annotations

from typing import Any

from erserver import queue


class _Cursor:
    def __init__(self, row: tuple | None) -> None:
        self._row = row
        self.sql: str | None = None
        self.params: tuple | None = None

    def __enter__(self) -> _Cursor:
        return self

    def __exit__(self, *exc: object) -> None:
        return None

    def execute(self, sql: str, params: tuple) -> None:
        self.sql, self.params = sql, params

    def fetchone(self) -> tuple | None:
        return self._row


class _Connection:
    def __init__(self, row: tuple | None) -> None:
        self.cursor_obj = _Cursor(row)

    def cursor(self) -> _Cursor:
        return self.cursor_obj


def test_stored_result_returns_the_persisted_record() -> None:
    record: dict[str, Any] = {"exit_code": 3, "error_class": "data", "job_id": "job-1"}
    connection = _Connection((record,))

    assert queue.stored_result(connection, "job-1") == record  # type: ignore[arg-type]
    assert connection.cursor_obj.params == ("job-1",)


def test_stored_result_rejects_missing_rows_and_malformed_payloads() -> None:
    assert queue.stored_result(_Connection(None), "job-1") is None  # type: ignore[arg-type]
    assert queue.stored_result(_Connection((None,)), "job-1") is None  # type: ignore[arg-type]
    assert queue.stored_result(_Connection(({"stages": []},)), "job-1") is None  # type: ignore[arg-type]
