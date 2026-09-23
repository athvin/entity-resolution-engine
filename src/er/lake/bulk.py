"""Bounded column batches for temporary DuckDB staging tables."""

from __future__ import annotations

from collections.abc import Iterable, Iterator, Sequence
from contextlib import contextmanager, suppress
from itertools import batched
from typing import Any
from uuid import uuid4

import duckdb

from er.entities.ids import IdFactory
from er.obs.profiling import span

# Amortize DuckDB binding/statement costs while retaining bounded Python pages.
BATCH_ROWS = 8192


@contextmanager
def staged_ids(
    connection: duckdb.DuckDBPyConnection, count: int, factory: IdFactory
) -> Iterator[str]:
    """Mint ordered IDs without reading the records they will be joined to."""
    with staged_rows(
        connection,
        (("position", "BIGINT"), ("id", "VARCHAR")),
        ((position, factory.new()) for position in range(1, count + 1)),
    ) as relation:
        yield relation


@contextmanager
def staged_query(
    connection: duckdb.DuckDBPyConnection,
    query: str,
    parameters: Sequence[Any] = (),
) -> Iterator[str]:
    """Materialize SQL in spillable local storage without transferring its rows."""
    relation = f"temp.main.er_bulk_{uuid4().hex}"
    connection.execute(f"CREATE TEMP TABLE {relation} AS {query}", list(parameters))
    try:
        yield relation
    except BaseException:
        with suppress(duckdb.Error):
            connection.execute(f"DROP TABLE {relation}")
        raise
    else:
        connection.execute(f"DROP TABLE {relation}")


def relation_pages(
    connection: duckdb.DuckDBPyConnection, relation: str, columns: str
) -> Iterator[list[tuple[Any, ...]]]:
    """Read an explicitly ordered, materialized relation in bounded ordinal ranges.

    The caller supplies a dense one-based ``position`` column. Unlike an open
    result cursor, the relation survives interleaved writes on this connection.
    """
    offset = 0
    while rows := connection.execute(
        f"SELECT {columns} FROM {relation} WHERE position > ? AND position <= ? ORDER BY position",
        [offset, offset + BATCH_ROWS],
    ).fetchall():
        yield rows
        offset += BATCH_ROWS


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
