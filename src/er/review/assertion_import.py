"""Native assertion-file loading with ordered conflict and prefix-commit semantics."""

from __future__ import annotations

import csv
from contextlib import ExitStack
from datetime import datetime
from pathlib import Path

import duckdb

from er.entities.ids import IdFactory
from er.lake.bulk import staged_ids, staged_query, staged_rows
from er.review.assertions import (
    _ASSERTIONS,
    _COLUMNS,
    ASSERTIONS_CSV_COLUMNS,
    ASSERTIONS_CSV_HEADER,
    Assertion,
    _add,
    _row_from,
    parse_assertions_csv,
)


def load_file(
    connection: duckdb.DuckDBPyConnection, path: Path, ids: IdFactory, stamp: datetime
) -> list[Assertion]:
    """Validate the whole file, then insert unique rows before the first conflict.

    Parsing errors write nothing. A write-time conflict commits the preceding
    valid prefix, including conflicts between rows in the same file. The returned
    objects are CLI output; input records and precedence checks remain in SQL.
    """
    with ExitStack() as stack:

        def stage(query: str, parameters: list[object] | None = None) -> str:
            return stack.enter_context(staged_query(connection, query, parameters or []))

        with path.open(encoding="utf-8") as handle:
            header = handle.readline().rstrip("\r\n")
        raw: str | None = None
        if header == ASSERTIONS_CSV_HEADER:
            try:
                raw = stage(
                    "SELECT * FROM read_csv(?, header=true, all_varchar=true, "
                    "force_not_null=?, delim=',', quote='\"', escape='\"', auto_detect=false, "
                    "columns={'phase':'VARCHAR','rec_a_key':'VARCHAR','rec_b_key':'VARCHAR',"
                    "'kind':'VARCHAR','created_by':'VARCHAR','note':'VARCHAR'}) WITH ORDINALITY",
                    [str(path), list(ASSERTIONS_CSV_COLUMNS)],
                )
                # read_text() in the reference parser applies universal newlines,
                # including within quoted fields. Apply the same normalization.
                raw = stage(
                    "SELECT "
                    + ", ".join(
                        f"replace(replace({name}, chr(13)||chr(10), chr(10)), "
                        f"chr(13), chr(10)) AS {name}"
                        for name in ASSERTIONS_CSV_COLUMNS
                    )
                    + f", ordinality FROM {raw}"
                )
                field_lengths = ", ".join(f"length({name})" for name in ASSERTIONS_CSV_COLUMNS)
                invalid = connection.execute(
                    f"SELECT 1 FROM {raw} WHERE phase NOT IN "
                    f"('base','batch','refresh','resurrect') "
                    f"OR greatest({field_lengths}) "
                    f"> {csv.field_size_limit()} "
                    "OR kind NOT IN ('always','never') OR created_by='' "
                    "OR NOT regexp_full_match(rec_a_key, '[^:]+:[^:]+', 's') "
                    "OR NOT regexp_full_match(rec_b_key, '[^:]+:[^:]+', 's') LIMIT 1"
                ).fetchone()
                if invalid is not None:
                    raw = None
            except duckdb.Error:
                raw = None
        if raw is None:
            # Rare dialect/diagnostic compatibility path. Full validation precedes
            # any write, just as in the public parser, and bulk persistence follows.
            rows = parse_assertions_csv(path)
            raw = stack.enter_context(
                staged_rows(
                    connection,
                    tuple((name, "VARCHAR") for name in ASSERTIONS_CSV_COLUMNS)
                    + (("ordinality", "BIGINT"),),
                    (
                        (r.phase, r.rec_a_key, r.rec_b_key, r.kind, r.created_by, r.note, i)
                        for i, r in enumerate(rows, 1)
                    ),
                )
            )
        canonical = stage(
            "SELECT ordinality AS position, least(rec_a_key, rec_b_key) AS rec_a_key, "
            "greatest(rec_a_key, rec_b_key) AS rec_b_key, kind, created_by, "
            f"nullif(note, '\\N') AS note FROM {raw}"
        )
        state = stage(
            "SELECT r.*, first_value(kind) OVER (PARTITION BY rec_a_key, rec_b_key "
            "ORDER BY position) AS first_kind, row_number() OVER "
            "(PARTITION BY rec_a_key, rec_b_key ORDER BY position) AS pair_position, "
            "coalesce(e.n, 0) AS existing_count, e.existing_kind "
            f"FROM {canonical} r LEFT JOIN (SELECT rec_a_key, rec_b_key, count(*) AS n, "
            f"min(kind) AS existing_kind FROM {_ASSERTIONS} WHERE active GROUP BY ALL) e "
            "USING(rec_a_key, rec_b_key)"
        )
        failure = connection.execute(
            f"SELECT position, rec_a_key, rec_b_key, kind, created_by, note FROM {state} "
            "WHERE rec_a_key=rec_b_key OR existing_count > 1 OR kind <> existing_kind "
            "OR kind <> first_kind ORDER BY position LIMIT 1"
        ).fetchone()
        limit = "TRUE" if failure is None else f"position < {int(failure[0])}"
        pending = stage(
            f"SELECT *, row_number() OVER (ORDER BY position) AS id_position FROM {state} "
            f"WHERE pair_position=1 AND existing_count=0 AND {limit}"
        )
        count = connection.execute(f"SELECT count(*) FROM {pending}").fetchone()
        assert count is not None
        generated = stack.enter_context(staged_ids(connection, int(count[0]), ids))
        written = stage(
            "SELECT i.id AS assertion_id, p.rec_a_key, p.rec_b_key, p.kind, true AS active, "
            "p.created_by, ?::TIMESTAMP AS created_at, NULL::VARCHAR AS retracted_by, "
            "NULL::TIMESTAMP AS retracted_at, p.note, p.position "
            f"FROM {pending} p JOIN {generated} i ON p.id_position=i.position",
            [stamp],
        )
        if int(count[0]):
            connection.execute("BEGIN TRANSACTION")
            try:
                connection.execute(
                    f"INSERT INTO {_ASSERTIONS} ({', '.join(_COLUMNS)}) "
                    f"SELECT {', '.join(_COLUMNS)} FROM {written} ORDER BY position"
                )
                connection.execute("COMMIT")
            except BaseException:
                connection.execute("ROLLBACK")
                raise
        if failure is not None:
            # Reuse the authoritative write-time diagnostic AFTER the prefix
            # commit; it also names an earlier row minted by this import.
            _add(
                connection,
                a=str(failure[1]),
                b=str(failure[2]),
                kind=str(failure[3]),
                created_by=str(failure[4]),
                note=failure[5],
                id_factory=ids,
                created_at=stamp,
            )
            raise AssertionError("conflicting assertion was not rejected")
        return [
            _row_from(row)
            for row in connection.execute(
                f"SELECT {', '.join(_COLUMNS)} FROM {written} ORDER BY position"
            ).fetchall()
        ]
