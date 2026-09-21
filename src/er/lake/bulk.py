"""Bounded column batches for temporary DuckDB staging tables."""

from __future__ import annotations

from collections.abc import Iterable, Sequence
from itertools import batched
from typing import Any

import duckdb

from er.obs.profiling import span

BATCH_ROWS = 1024


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
