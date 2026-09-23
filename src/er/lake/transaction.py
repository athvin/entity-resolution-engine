"""Transactions owned by a stage's atomic apply step."""

from collections.abc import Iterator
from contextlib import contextmanager

import duckdb


@contextmanager
def transaction(connection: duckdb.DuckDBPyConnection) -> Iterator[None]:
    """Commit all writes together, including rollback on cancellation."""
    connection.execute("BEGIN TRANSACTION")
    try:
        yield
        connection.execute("COMMIT")
    except BaseException:
        connection.execute("ROLLBACK")
        raise
