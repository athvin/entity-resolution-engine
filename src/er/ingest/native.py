"""File-to-relation ingestion, with explicit compatibility fallbacks."""

from __future__ import annotations

import csv
from collections.abc import Sequence
from pathlib import Path

import duckdb

from er.ingest.hashing import content_hash_sql
from er.ingest.sources import CsvAdapter, ParquetAdapter
from er.lake.bulk import staged_query
from er.obs.profiling import profiled


def _identifier(name: str) -> str:
    return '"' + name.replace('"', '""') + '"'


def _literal(value: str) -> str:
    return "'" + value.replace("'", "''") + "'"


def _projection(names: Sequence[str], types: Sequence[str], date_format: str) -> list[str] | None:
    expressions = []
    for name, kind in zip(names, types, strict=True):
        column = _identifier(name)
        if kind == "VARCHAR":
            value = column
        elif kind in {
            "BOOLEAN",
            "TINYINT",
            "SMALLINT",
            "INTEGER",
            "BIGINT",
            "UTINYINT",
            "USMALLINT",
            "UINTEGER",
            "UBIGINT",
            "HUGEINT",
            "UHUGEINT",
        }:
            value = (
                f"CASE WHEN {column} THEN 'True' WHEN NOT {column} THEN 'False' END"
                if kind == "BOOLEAN"
                else f"CAST({column} AS VARCHAR)"
            )
        elif kind == "DATE" and date_format in {"%Y-%m-%d", "%m/%d/%Y", "%d/%m/%Y"}:
            value = f"strftime({column}, {_literal(date_format)})"
        elif kind in {"TIMESTAMP", "TIMESTAMP_MS", "TIMESTAMP_S"}:
            value = (
                f"strftime({column}, '%Y-%m-%dT%H:%M:%S') || "
                f"CASE WHEN microsecond({column}) % 1000000 = 0 THEN '' "
                f"ELSE '.' || strftime({column}, '%f') END"
            )
        else:
            # Python's float, decimal, timezone and nested-value rendering is a
            # compatibility contract, not equivalent to a blanket VARCHAR cast.
            return None
        expressions.append(f"{value} AS {column}")
    return expressions


@profiled("ingest.native_file", "records")
def stage_file(
    connection: duckdb.DuckDBPyConnection,
    adapter: CsvAdapter | ParquetAdapter,
    path: Path,
    destination: str,
    offset: int,
) -> int | None:
    """Stage a supported file, or return None to request the streaming adapter.

    Only headers/schema and one invalid-key diagnostic can leave DuckDB. Native
    parse failures are retried through the existing parser by the caller so its
    accepted dialect and file/row error messages remain authoritative.
    """
    if not adapter.source or ":" in adapter.source:
        return None
    if isinstance(adapter, CsvAdapter):
        with path.open(newline="", encoding="utf-8") as handle:
            names = next(csv.reader(handle), None)
        if names is None:
            return 0
        adapter._require_record_id_column(path, names)
        if len({name.casefold() for name in names}) != len(names) or any(
            not name for name in names
        ):
            return None
        types = ["VARCHAR"] * len(names)
        scan = (
            "read_csv(?, header=true, all_varchar=true, force_not_null=?, "
            "delim=',', quote='\"', escape='\"', auto_detect=false)"
        )
        # Explicit column names avoid dialect/type inference and preserve headers.
        scan = (
            scan[:-1]
            + ", columns={"
            + ", ".join(f"{_literal(name)}: 'VARCHAR'" for name in names)
            + "})"
        )
        parameters: list[object] = [str(path), names]
    else:
        schema = connection.execute(
            "DESCRIBE SELECT * FROM read_parquet(?)", [str(path)]
        ).fetchall()
        names, types = [str(row[0]) for row in schema], [str(row[1]) for row in schema]
        adapter._require_record_id_column(path, names)
        scan, parameters = "read_parquet(?)", [str(path)]
    if any(name.casefold() == "ordinality" for name in names):
        return None
    expressions = _projection(names, types, adapter.spec.date_format)
    if expressions is None:
        return None
    date_columns = [
        _identifier(name)
        for name, kind in zip(names, types, strict=True)
        if kind in {"DATE", "TIMESTAMP", "TIMESTAMP_MS", "TIMESTAMP_S"}
    ]
    if date_columns:
        # Python's year rendering (and its out-of-range date conversion) differs
        # from SQL strftime outside this range. Preserve the adapter on that seam.
        unusual = " OR ".join(
            f"NOT isfinite({column}) OR year({column}) NOT BETWEEN 1000 AND 9999"
            for column in date_columns
        )
        if connection.execute(
            f"SELECT EXISTS (SELECT 1 FROM {scan} WHERE {unusual})", parameters
        ).fetchone() == (True,):
            return None
    with staged_query(
        connection,
        f"SELECT {', '.join(expressions)}, ordinality FROM {scan} WITH ORDINALITY",
        parameters,
    ) as delivered:
        if isinstance(adapter, CsvAdapter):
            too_wide = " OR ".join(
                f"length({_identifier(name)}) > {csv.field_size_limit()}" for name in names
            )
            if connection.execute(
                f"SELECT EXISTS (SELECT 1 FROM {delivered} WHERE {too_wide})"
            ).fetchone() == (True,):
                return None
        key = _identifier(adapter.spec.record_id_column)
        invalid = connection.execute(
            f"SELECT ordinality, {key} FROM {delivered} WHERE {key} IS NULL "
            f"OR {key} = '' OR contains({key}, ':') ORDER BY ordinality LIMIT 1"
        ).fetchone()
        if invalid is not None:
            # Replay only on the exceptional path to preserve the original error.
            return None
        fields = ", ".join(f"{_literal(name)}, {_identifier(name)}" for name in names)
        digest = content_hash_sql(
            adapter.columns, names, metadata_columns=adapter.spec.metadata_columns(names)
        )
        count = connection.execute(
            f"INSERT INTO {destination} SELECT ordinality + ?, ?, {key}, "
            f"json_object({fields})::VARCHAR, {digest} FROM {delivered} ORDER BY ordinality",
            [offset, adapter.source],
        ).fetchone()
        assert count is not None
        return int(count[0])
