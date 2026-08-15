"""Unit tests for the S4.1 drop-folder source adapters (DesignDoc.md S4.1, S4.7, S6).

The headline assertion is the CSV/Parquet one: the same ten records delivered in
either format must project to the same ``values`` tuple and hash to the same
``content_hash``, because S4.1 defines the digest over the delivered source
columns and a delivery format is not a content change.

The test data is written as the **text a CSV delivery carries**, and the Parquet
fixture is built by *parsing* that text into typed values. The oracle therefore
runs in the opposite direction from the module under test, which renders typed
values back to text: a shared formatting helper would agree with any rewrite of
the renderer, including a wrong one.
"""

from __future__ import annotations

import csv
import datetime as dt
import io
from collections.abc import Iterator, Mapping, Sequence
from pathlib import Path

import duckdb
import pytest

from er.config.loader import load_config
from er.config.schema import Config
from er.errors import ConfigError, ErrorClass, ExitCode, StageFailure
from er.ingest.hashing import content_hash
from er.ingest.sources import (
    CsvAdapter,
    ParquetAdapter,
    SourceAdapter,
    SourceRow,
    adapter_for,
    discover_files,
)

REPO_ROOT = Path(__file__).resolve().parents[2]
CONFIG_PATH = REPO_ROOT / "configs" / "test.yaml"

# One `sources.crm` delivery, written as the source system would write it: the key
# column, the nine V11 canonical attributes in their declared order, the
# updated_at column, and `notes`, which the `columns` block does not name and
# which is therefore what proves `payload` is verbatim rather than a
# re-serialization of the projection.
CRM_DELIVERY_CSV = """\
crm_id,first_name,last_name,email_address,phone,street_address,city,state,zip,dob,last_modified,notes
c-001,Ann,Lee,ann@e.com,5550101,1 Oak St,Spring,IL,62701,02/03/1990,2024-01-02T03:04:05,vip
c-002,Bob,Chen,bob@e.com,5550102,2 Elm St,Spring,IL,62702,11/30/1985,2024-01-03T09:15:00,-
c-003,Cara,Diaz,cara@e.com,5550103,3 Ash St,Shelby,IL,62565,06/15/1978,2024-01-04T11:00:59,-
c-004,Dan,Ellis,dan@e.com,5550104,4 Fir St,Shelby,IL,62565,01/01/2001,2024-01-05T23:59:58,-
c-005,Eve,Fox,eve@e.com,5550105,5 Gum St,Ogden,IL,62801,09/09/1996,2024-01-06T00:00:01,-
c-006,Finn,Gray,finn@e.com,5550106,6 Ivy St,Ogden,IL,62801,12/25/1969,2024-01-07T13:45:30,-
c-007,Gita,Hall,gita@e.com,5550107,7 Yew St,Capital,IL,62701,03/17/1988,2024-01-08T07:07:07,-
c-008,Hugo,Iqbal,hugo@e.com,5550108,8 Bay St,Capital,IL,62701,07/04/1993,2024-01-09T18:22:11,-
c-009,Ivy,Jones,ivy@e.com,5550109,9 Ken St,Haverbrook,IL,62901,04/22/1975,2024-01-10T05:05:05,-
c-010,Jack,Kaur,jack@e.com,5550110,10 Law St,Haverbrook,IL,62901,10/08/2003,2024-01-11T21:30:00,-
"""

# `billing`'s format from S6, not `crm`'s. `crm`'s "%Y-%m-%d" is character-for-
# character what `date.isoformat()` produces, so a Parquet path that ignored
# `date_format` entirely would still pass the equality assertion -- the test would
# assert nothing. This one fails unless the configured format is really applied.
DELIVERY_DATE_FORMAT = "%m/%d/%Y"

_DELIVERY_ROWS: list[list[str]] = list(csv.reader(io.StringIO(CRM_DELIVERY_CSV)))
CRM_HEADER: tuple[str, ...] = tuple(_DELIVERY_ROWS[0])
TEN_RECORDS: tuple[tuple[str, ...], ...] = tuple(tuple(row) for row in _DELIVERY_ROWS[1:])

# `sources.crm.columns` in declaration order -- the order `values` follows and the
# `columns` argument of `content_hash`.
CRM_DECLARED_COLUMNS: tuple[str, ...] = (
    "first_name",
    "last_name",
    "email_address",
    "phone",
    "street_address",
    "city",
    "state",
    "zip",
    "dob",
)

# The typed columns only Parquet can carry, and how the text spelling above is
# parsed back into them. Everything else is delivered and stored as VARCHAR.
PARQUET_COLUMN_TYPES: Mapping[str, str] = {"dob": "DATE", "last_modified": "TIMESTAMP"}


def write_csv(
    path: Path,
    rows: Sequence[Sequence[str]],
    header: Sequence[str] = CRM_HEADER,
) -> None:
    """Write a CSV delivery exactly as a source system would."""
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", newline="", encoding="utf-8") as handle:
        writer = csv.writer(handle)
        writer.writerow(header)
        writer.writerows(rows)


def to_typed(row: Sequence[str], header: Sequence[str] = CRM_HEADER) -> tuple[object, ...]:
    """Parse one text row into the typed values a Parquet delivery carries."""
    typed: list[object] = []
    for name, value in zip(header, row, strict=True):
        column_type = PARQUET_COLUMN_TYPES.get(name)
        if column_type == "DATE":
            typed.append(dt.datetime.strptime(value, DELIVERY_DATE_FORMAT).date())
        elif column_type == "TIMESTAMP":
            typed.append(dt.datetime.fromisoformat(value))
        else:
            # An empty CSV field becomes a real Parquet NULL, which is the one
            # asymmetry the two delivery formats have.
            typed.append(value or None)
    return tuple(typed)


def write_parquet(
    path: Path,
    rows: Sequence[Sequence[str]],
    header: Sequence[str] = CRM_HEADER,
) -> None:
    """Write the same records as Parquet, with `dob` DATE and `last_modified` TIMESTAMP."""
    path.parent.mkdir(parents=True, exist_ok=True)
    columns = ", ".join(f'"{name}" {PARQUET_COLUMN_TYPES.get(name, "VARCHAR")}' for name in header)
    placeholders = ", ".join("?" for _ in header)
    # COPY TO takes no bound parameter, so the destination is a SQL literal with
    # embedded quotes doubled -- the same discipline as S4.0b's ATTACH.
    destination = str(path).replace("'", "''")
    connection = duckdb.connect()
    try:
        connection.execute(f"CREATE TABLE delivery ({columns})")
        if rows:
            connection.executemany(
                f"INSERT INTO delivery VALUES ({placeholders})",
                [to_typed(row, header) for row in rows],
            )
        connection.execute(f"COPY delivery TO '{destination}' (FORMAT parquet)")
    finally:
        connection.close()


@pytest.fixture(scope="session")
def reference_config() -> Config:
    """`configs/test.yaml`, the document the fixtures and CI use verbatim (S6)."""
    return load_config(CONFIG_PATH)


@pytest.fixture
def config(reference_config: Config, tmp_path: Path) -> Config:
    """The reference document, rooted at a temporary drop dir, plus two extra sources.

    `crm_parquet` is `crm` delivered as Parquet -- same mapping, same
    `date_format`, so the CSV/Parquet equality case differs in the delivery format
    and in nothing else. `crm_spreadsheet` carries an adapter token v1 does not
    implement.
    """
    document = reference_config.model_copy(deep=True)
    document.storage.drop_dir = str(tmp_path)
    document.sources["crm"].date_format = DELIVERY_DATE_FORMAT
    parquet_source = document.sources["crm"].model_copy(deep=True)
    parquet_source.adapter = "parquet"
    document.sources["crm_parquet"] = parquet_source
    unknown_adapter = document.sources["crm"].model_copy(deep=True)
    unknown_adapter.adapter = "spreadsheet"
    document.sources["crm_spreadsheet"] = unknown_adapter
    return document


def digest(adapter: SourceAdapter, row: SourceRow) -> str:
    """The S4.1 `content_hash` of a row, computed from the projected tuple."""
    return content_hash(dict(zip(adapter.columns, row.values, strict=True)), adapter.columns)


def drain(rows: Iterator[SourceRow]) -> list[SourceRow]:
    return list(rows)


def test_discovery_and_row_order_are_deterministic(config: Config, tmp_path: Path) -> None:
    """AC1: files sort by name, and two full iterations are identical."""
    # Created out of alphabetical order, so a filesystem-order implementation
    # cannot pass by accident.
    write_csv(tmp_path / "crm" / "c_third.csv", TEN_RECORDS[6:])
    write_csv(tmp_path / "crm" / "a_first.csv", TEN_RECORDS[:3])
    write_csv(tmp_path / "crm" / "b_second.csv", TEN_RECORDS[3:6])
    (tmp_path / "crm" / "ignored.txt").write_text("not a delivery", encoding="utf-8")

    discovered = discover_files(tmp_path, "crm", (".csv",))
    assert [path.name for path in discovered] == ["a_first.csv", "b_second.csv", "c_third.csv"]

    adapter = adapter_for(config, "crm")
    assert isinstance(adapter, SourceAdapter)
    assert adapter.discover() == discovered

    first = drain(adapter.rows())
    second = drain(adapter.rows())
    assert first == second
    # File order, then in-file order: the three files hold the ten records in
    # their original sequence.
    assert [row.source_record_id for row in first] == [record[0] for record in TEN_RECORDS]


def test_projection_follows_declared_column_order(config: Config, tmp_path: Path) -> None:
    """AC2: `values` is the declared mapping order; absent mapped columns are None."""
    write_csv(tmp_path / "crm" / "delivery.csv", TEN_RECORDS[:1])
    adapter = adapter_for(config, "crm")
    assert adapter.columns == CRM_DECLARED_COLUMNS

    (row,) = drain(adapter.rows())
    assert row.source_system == "crm"
    assert row.source_record_id == "c-001"
    assert row.values == tuple(
        TEN_RECORDS[0][CRM_HEADER.index(column)] for column in CRM_DECLARED_COLUMNS
    )

    # A mapped column the delivery omits entirely projects as None -- which
    # `content_hash` encodes as the empty string, so the record still hashes.
    trimmed_header = tuple(name for name in CRM_HEADER if name != "phone")
    trimmed_rows = [
        tuple(value for name, value in zip(CRM_HEADER, record, strict=True) if name != "phone")
        for record in TEN_RECORDS[:1]
    ]
    write_csv(tmp_path / "crm" / "delivery.csv", trimmed_rows, trimmed_header)
    (sparse,) = drain(adapter_for(config, "crm").rows())
    assert sparse.values[CRM_DECLARED_COLUMNS.index("phone")] is None


def test_payload_is_verbatim_including_unmapped_columns(config: Config, tmp_path: Path) -> None:
    """AC2: `payload` carries every delivered column, unmapped ones included."""
    write_csv(tmp_path / "crm" / "delivery.csv", TEN_RECORDS[:2])
    rows = drain(adapter_for(config, "crm").rows())

    for row, record in zip(rows, TEN_RECORDS[:2], strict=True):
        assert dict(row.payload) == dict(zip(CRM_HEADER, record, strict=True))
    # `notes` and `last_modified` are outside `columns`, so they reach staging
    # through `payload` and are absent from the hashed tuple.
    assert "notes" not in CRM_DECLARED_COLUMNS
    assert "last_modified" not in CRM_DECLARED_COLUMNS
    assert rows[0].payload["notes"] == "vip"
    assert rows[0].payload["last_modified"] == "2024-01-02T03:04:05"


def test_csv_and_parquet_hash_identically(config: Config, tmp_path: Path) -> None:
    """AC3: the same ten records in either format project and hash identically."""
    write_csv(tmp_path / "crm" / "delivery.csv", TEN_RECORDS)
    write_parquet(tmp_path / "crm_parquet" / "delivery.parquet", TEN_RECORDS)

    csv_adapter = adapter_for(config, "crm")
    parquet_adapter = adapter_for(config, "crm_parquet")
    assert isinstance(csv_adapter, CsvAdapter)
    assert isinstance(parquet_adapter, ParquetAdapter)

    csv_rows = drain(csv_adapter.rows())
    parquet_rows = drain(parquet_adapter.rows())
    assert len(csv_rows) == len(parquet_rows) == len(TEN_RECORDS)

    for from_csv, from_parquet in zip(csv_rows, parquet_rows, strict=True):
        # The DATE column went through `sources.crm.date_format` and the TIMESTAMP
        # through ISO-8601 to get back to the text the CSV delivered.
        assert from_parquet.values == from_csv.values
        assert dict(from_parquet.payload) == dict(from_csv.payload)
        assert digest(parquet_adapter, from_parquet) == digest(csv_adapter, from_csv)

    # Spelled out rather than left implicit in the equality above: the DATE really
    # is rendered through the configured format and not through `date.isoformat`.
    assert parquet_rows[0].payload["dob"] == "02/03/1990"
    assert parquet_rows[0].payload["last_modified"] == "2024-01-02T03:04:05"

    # The one place the two formats legitimately differ: a CSV has no NULL, so an
    # empty field renders as '' and a Parquet NULL renders as None. S4.1 encodes
    # both as the empty string, so the digests still agree -- and that, not the
    # rendered value, is what the anti-join append compares.
    missing_phone = list(TEN_RECORDS[0])
    missing_phone[CRM_HEADER.index("phone")] = ""
    write_csv(tmp_path / "crm" / "delivery.csv", [tuple(missing_phone)])
    write_parquet(tmp_path / "crm_parquet" / "delivery.parquet", [tuple(missing_phone)])
    (sparse_csv,) = drain(adapter_for(config, "crm").rows())
    (sparse_parquet,) = drain(adapter_for(config, "crm_parquet").rows())

    position = CRM_DECLARED_COLUMNS.index("phone")
    assert sparse_csv.values[position] == ""
    assert sparse_parquet.values[position] is None
    assert digest(csv_adapter, sparse_csv) == digest(parquet_adapter, sparse_parquet)


def test_unknown_source_and_adapter_exit_2(config: Config) -> None:
    """AC4: both are S4.7 `config`, exit 2, and name the offending key."""
    with pytest.raises(ConfigError) as unknown_source:
        adapter_for(config, "warehouse")
    assert unknown_source.value.code == ExitCode.CONFIG
    assert unknown_source.value.error_class is ErrorClass.CONFIG
    assert "sources.warehouse" in str(unknown_source.value)

    with pytest.raises(ConfigError) as unknown_adapter:
        adapter_for(config, "crm_spreadsheet")
    assert unknown_adapter.value.code == ExitCode.CONFIG
    assert unknown_adapter.value.error_class is ErrorClass.CONFIG
    assert "sources.crm_spreadsheet.adapter" in str(unknown_adapter.value)
    assert "spreadsheet" in str(unknown_adapter.value)


def test_missing_record_id_column_is_data_error(config: Config, tmp_path: Path) -> None:
    """AC5: a header without `record_id_column` fails before any row is yielded."""
    header = tuple(name for name in CRM_HEADER if name != "crm_id")
    rows = [
        tuple(value for name, value in zip(CRM_HEADER, record, strict=True) if name != "crm_id")
        for record in TEN_RECORDS[:3]
    ]
    delivery = tmp_path / "crm" / "delivery.csv"
    write_csv(delivery, rows, header)

    iterator = adapter_for(config, "crm").rows()
    with pytest.raises(StageFailure) as failure:
        next(iterator)
    assert failure.value.code == ExitCode.STAGE_FAILURE
    assert failure.value.error_class is ErrorClass.DATA
    assert str(delivery) in str(failure.value)
    assert "crm_id" in str(failure.value)


def test_colon_in_source_record_id_is_data_error(config: Config, tmp_path: Path) -> None:
    """AC6: S4.7's `data` example, named down to the file and the row number."""
    poisoned = list(TEN_RECORDS[1])
    poisoned[CRM_HEADER.index("crm_id")] = "c:002"
    delivery = tmp_path / "crm" / "delivery.csv"
    write_csv(delivery, [TEN_RECORDS[0], tuple(poisoned), TEN_RECORDS[2]])

    iterator = adapter_for(config, "crm").rows()
    # The clean row before it still arrives: the failure is the row's, not the file's.
    assert next(iterator).source_record_id == "c-001"
    with pytest.raises(StageFailure) as failure:
        next(iterator)
    assert failure.value.code == ExitCode.STAGE_FAILURE
    assert failure.value.error_class is ErrorClass.DATA
    assert str(delivery) in str(failure.value)
    assert "row 2" in str(failure.value)
    assert "':'" in str(failure.value)


def test_empty_delivery_yields_zero_rows(config: Config, tmp_path: Path) -> None:
    """AC7: no directory, no files, and a header-only file all yield zero rows."""
    adapter = adapter_for(config, "crm")
    assert adapter.discover() == ()
    assert drain(adapter.rows()) == []

    (tmp_path / "crm").mkdir()
    assert drain(adapter_for(config, "crm").rows()) == []

    write_csv(tmp_path / "crm" / "header_only.csv", [])
    assert drain(adapter_for(config, "crm").rows()) == []

    # A zero-byte file has no header at all and is still not a defect.
    (tmp_path / "crm" / "zero_bytes.csv").write_bytes(b"")
    assert drain(adapter_for(config, "crm").rows()) == []

    write_parquet(tmp_path / "crm_parquet" / "empty.parquet", [])
    assert drain(adapter_for(config, "crm_parquet").rows()) == []


def test_rows_is_lazy(config: Config, tmp_path: Path) -> None:
    """AC8: the first row arrives without the unparsable tail being read."""
    delivery = tmp_path / "crm" / "delivery.csv"
    delivery.parent.mkdir(parents=True, exist_ok=True)
    padding = ",".join("pad" for _ in CRM_HEADER).encode("utf-8") + b"\n"
    with delivery.open("wb") as handle:
        handle.write(",".join(CRM_HEADER).encode("utf-8") + b"\n")
        handle.write(",".join(TEN_RECORDS[0]).encode("utf-8") + b"\n")
        # Far beyond one decoder chunk, so reaching the invalid bytes below means
        # the reader really did stream to the end of the file.
        handle.write(padding * 2000)
        handle.write(b"c-999,\xff\xfe not utf-8\n")

    iterator = adapter_for(config, "crm").rows()
    assert next(iterator).source_record_id == "c-001"

    # And the tail is genuinely unparsable, so the first row above was not a lucky
    # read of an entirely valid file.
    with pytest.raises(StageFailure) as failure:
        drain(iterator)
    assert failure.value.error_class is ErrorClass.DATA
    assert str(delivery) in str(failure.value)


def test_corrupt_file_is_data_error(config: Config, tmp_path: Path) -> None:
    """AC7: an unparsable file is S4.7 `data`, exit 1, naming the file."""
    corrupt = tmp_path / "crm_parquet" / "corrupt.parquet"
    corrupt.parent.mkdir(parents=True, exist_ok=True)
    corrupt.write_bytes(b"PAR0 this is not a parquet file" * 8)

    with pytest.raises(StageFailure) as failure:
        drain(adapter_for(config, "crm_parquet").rows())
    assert failure.value.code == ExitCode.STAGE_FAILURE
    assert failure.value.error_class is ErrorClass.DATA
    assert str(corrupt) in str(failure.value)


def test_ragged_row_is_data_error(config: Config, tmp_path: Path) -> None:
    """A row with the wrong field count would misalign every value after the gap."""
    delivery = tmp_path / "crm" / "delivery.csv"
    write_csv(delivery, [TEN_RECORDS[0]])
    with delivery.open("a", newline="", encoding="utf-8") as handle:
        handle.write("c-002,Bob,Chen\n")

    with pytest.raises(StageFailure) as failure:
        drain(adapter_for(config, "crm").rows())
    assert failure.value.code == ExitCode.STAGE_FAILURE
    assert failure.value.error_class is ErrorClass.DATA
    assert "row 2" in str(failure.value)
