"""Bounded column batches for temporary DuckDB staging tables."""

from __future__ import annotations

from collections.abc import Iterable, Iterator, Sequence
from contextlib import contextmanager, suppress
from itertools import batched
from typing import Any
from uuid import uuid4

import duckdb

from er.obs.profiling import span

BATCH_ROWS = 1024


@contextmanager
def staged_rows(
    connection: duckdb.DuckDBPyConnection,
    schema: Sequence[tuple[str, str]],
    rows: Iterable[Sequence[Any]],
) -> Iterator[str]:
    """Load a local temporary relation in bounded batches, then remove it.

    Column names and SQL types are trusted application constants. Only data is
    bound. The explicit temp catalog keeps staging out of DuckLake regardless of
    the current schema, and the caller retains ownership of its transaction.
    """
    relation = f"temp.main.er_bulk_{uuid4().hex}"
    columns = ", ".join(f"{name} {sql_type}" for name, sql_type in schema)
    connection.execute(f"CREATE TEMP TABLE {relation} ({columns})")
    try:
        projection = ", ".join(f"unnest(?::{sql_type}[])" for _, sql_type in schema)
        insert_batches(
            connection,
            f"INSERT INTO {relation} SELECT {projection}",
            rows,
            columns=len(schema),
        )
        yield relation
    except BaseException:
        # An aborted caller transaction may refuse cleanup until rollback. Preserve
        # its original failure; never commit/roll back a transaction we do not own.
        with suppress(duckdb.Error):
            connection.execute(f"DROP TABLE {relation}")
        raise
    else:
        connection.execute(f"DROP TABLE {relation}")


def insert_batches(
    connection: duckdb.DuckDBPyConnection,
    statement: str,
    rows: Iterable[Sequence[Any]],
    *,
    columns: int,
    batch_rows: int = BATCH_ROWS,
) -> int:
    """Execute a fixed INSERT/UNNEST statement over equally sized bound columns.

    SQL and types belong to the caller; values never become SQL text. Batches are
    built from complete rows so UNNEST cannot silently pad a short column with NULL.
    """
    total = 0
    with span("sql.bulk_load", unit="rows") as metrics:
        batches = 0
        for batch in batched(rows, batch_rows):
            if any(len(row) != columns for row in batch):
                raise ValueError("bulk row has an unexpected number of columns")
            parameters = [list(values) for values in zip(*batch, strict=True)]
            connection.execute(statement, parameters)
            total += len(batch)
            batches += 1
        metrics.update(rows_in=total, rows_out=total, batches=batches)
    return total
