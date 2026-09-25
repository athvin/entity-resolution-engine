"""File scans retain adapter rendering, hashing, ordering and failure behavior."""

import csv
import json
from pathlib import Path

import duckdb
import pytest

from er.config.loader import load_config
from er.errors import StageFailure
from er.ingest.hashing import content_hash
from er.ingest.landing import STAGING_TABLE, _stage_files
from er.ingest.sources import CsvAdapter, ParquetAdapter

CONFIG = load_config(Path(__file__).resolve().parents[2] / "configs/test.yaml")


def check_rows(connection: duckdb.DuckDBPyConnection, adapter: CsvAdapter | ParquetAdapter) -> None:
    expected = list(adapter.rows())
    assert _stage_files(connection, adapter) == len(expected)
    rows = connection.execute(f"SELECT * FROM {STAGING_TABLE} ORDER BY seq").fetchall()
    for index, (actual, reference) in enumerate(zip(rows, expected, strict=True), 1):
        seq, source, key, payload, digest = actual
        assert (seq, source, key) == (index, reference.source_system, reference.source_record_id)
        assert json.loads(payload) == reference.payload
        assert digest == content_hash(
            reference.payload, adapter.columns, metadata_columns=tuple(reference.metadata)
        )


@pytest.mark.parametrize(
    "header,rows",
    [
        (
            ["crm_id", "first_name", "notes"],
            [
                ["1", "e\u0301", "quote, ' and \" 雪"],
                ["2", "", "line\r\nnext\nline"],
                ["3", "\\N", "unit\x1fseparator"],
            ],
        ),
        (["crm_id", "first_name", "first_name"], [["1", "before", "after"]]),
        (["crm_id", "first_name", "FIRST_NAME"], [["1", "before", "after"]]),
        (["crm_id", "ordinality", ""], [["1", "source ordinal", "empty header"]]),
    ],
)
def test_csv_values_and_compatibility_headers(
    tmp_path: Path, header: list[str], rows: list[list[str]]
) -> None:
    source = tmp_path / "crm"
    source.mkdir()
    for name in ("a.csv", "b.csv"):
        with (source / name).open("w", encoding="utf-8", newline="") as handle:
            writer = csv.writer(handle)
            writer.writerow(header)
            writer.writerows(rows)
    with duckdb.connect() as connection:
        check_rows(connection, CsvAdapter("crm", CONFIG.sources["crm"], tmp_path))


@pytest.mark.parametrize(
    "extra",
    [
        "true AS flag",
        "1.0e-7::DOUBLE AS value",
        "TIMESTAMP '2026-01-02 03:04:05.006' AS modified",
        "DATE '1990-02-03' AS dob",
        "DATE 'infinity' AS dob",
        "DATE '0001-01-01' AS dob",
        "[1,2,3] AS values",
    ],
)
def test_parquet_native_types_and_rendering_fallbacks(tmp_path: Path, extra: str) -> None:
    source = tmp_path / "crm"
    source.mkdir()
    with duckdb.connect() as connection:
        connection.execute(
            f"COPY (SELECT '1' AS crm_id, 'é' AS first_name, {extra}) TO ? (FORMAT PARQUET)",
            [str(source / "data.parquet")],
        )
        check_rows(connection, ParquetAdapter("crm", CONFIG.sources["crm"], tmp_path))


@pytest.mark.parametrize(
    "text",
    [
        "crm_id,first_name\n:bad,A\n",
        "crm_id,first_name\n1,A,extra\n",
        'crm_id,first_name\n1,"loose"quote\n',
        'crm_id,first_name\n1,"unterminated\n',
    ],
)
def test_csv_parser_compatibility_and_diagnostics(tmp_path: Path, text: str) -> None:
    source = tmp_path / "crm"
    source.mkdir()
    (source / "input.csv").write_text(text)
    adapter = CsvAdapter("crm", CONFIG.sources["crm"], tmp_path)
    try:
        list(adapter.rows())
    except StageFailure as expected:
        with duckdb.connect() as connection, pytest.raises(StageFailure) as actual:
            _stage_files(connection, adapter)
        assert str(actual.value) == str(expected)
    else:
        with duckdb.connect() as connection:
            check_rows(connection, adapter)


def test_native_csv_retains_python_field_limit(tmp_path: Path) -> None:
    source = tmp_path / "crm"
    source.mkdir()
    (source / "wide.csv").write_text("crm_id,first_name\n1," + "x" * (csv.field_size_limit() + 1))
    adapter = CsvAdapter("crm", CONFIG.sources["crm"], tmp_path)
    with duckdb.connect() as connection, pytest.raises(StageFailure, match="field larger"):
        _stage_files(connection, adapter)
