"""Drop-directory source adapters for ingest (DesignDoc.md S4.1, S4.7, S6).

S4.1 reads CSV/Parquet from ``storage.drop_dir/<source>/`` behind a
:class:`SourceAdapter` interface, taking the canonical->source column mapping,
``record_id_column``, ``updated_at_column`` and ``date_format`` from
``sources.<name>`` in S6. This module is that adapter layer and nothing else: it
produces the rows, and ER-031 decides what the lake does with them.

Five properties are load-bearing and are asserted rather than assumed:

* **Values are projected verbatim.** No trimming, no case folding, no date
  reformatting of a string the source delivered. S4.1 defines ``content_hash``
  over the delivered source columns, so a normalization here would make a
  reformatted redelivery hash as *unchanged* and the anti-join append would drop
  a real correction.
* **``date_format`` does exactly one job in this layer**: rendering a typed
  Parquet date to the same text a CSV delivery carries, so the two paths project
  and hash identically. It never reformats a string input. Standardization
  (``parse_date`` and the rest of S4.2) runs in dbt, after the hash.
* **``payload`` is the full source row**, including columns the ``columns`` block
  does not name -- S4.1 requires it verbatim and the S4.2 staging models extract
  from it, which is also why ``updated_at_column`` gets no field of its own here.
* **Order is deterministic.** Files sort by name and rows follow file order, so
  S4.1's anti-join append and the five ``ingest_batches`` counts are reproducible
  by construction rather than by luck.
* **:meth:`rows` is lazy.** A delivery at benchmark scale (S10.2) must never be
  materialized, so every reader here streams and the first row is produced before
  the last one is read.

Errors follow S4.7: an unknown source or adapter token is ``config`` (exit ``2``,
raised before a file is touched); an unreadable or unparsable file, a header
missing ``record_id_column``, and a ``source_record_id`` containing ``':'`` are
all ``data`` (exit ``1``), named down to the file and the row.

Parquet is read through the pinned ``duckdb`` (S2.1) rather than a new Arrow
dependency, on a private ``:memory:`` connection that never attaches the lake --
S4.0b's connection model governs lake access, and this reads a file on disk.
"""

from __future__ import annotations

import csv
import datetime as dt
import os
from collections.abc import Collection, Iterator, Mapping, Sequence
from dataclasses import dataclass
from pathlib import Path
from types import MappingProxyType
from typing import ClassVar, Protocol, runtime_checkable

import duckdb

from er.config.schema import Config, SourceSpec
from er.entities.ids import InvalidRecordKeyError, record_key
from er.errors import ConfigError, ErrorClass, StageFailure

__all__ = [
    "CSV_EXTENSIONS",
    "PARQUET_EXTENSIONS",
    "CsvAdapter",
    "ParquetAdapter",
    "SourceAdapter",
    "SourceRow",
    "adapter_for",
    "discover_files",
]

#: Extensions each v1 adapter claims in the drop directory. Compared against a
#: lowercased suffix, so ``CRM.CSV`` from a case-preserving upload is discovered.
CSV_EXTENSIONS: tuple[str, ...] = (".csv",)
PARQUET_EXTENSIONS: tuple[str, ...] = (".parquet",)

#: Rows pulled from DuckDB per :meth:`fetchmany`. Large enough that per-call
#: overhead disappears at S10.2's million-row scale, small enough that a delivery
#: is never resident -- which is the laziness property, not a tuning knob.
PARQUET_BATCH_ROWS = 1024


@dataclass(frozen=True, slots=True)
class SourceRow:
    """One delivered record, as S4.1's ingest write consumes it.

    Attributes:
        source_system: the ``sources.<name>`` key that delivered the row.
        source_record_id: the value of ``sources.<name>.record_id_column``,
            already checked against the S5.0 key model.
        payload: the full source row, every delivered column, rendered to text.
            S4.1's ``payload JSON``.
        values: the projection of ``sources.<name>.columns`` **in the declared
            order** -- the input tuple ``content_hash`` (S4.1) is defined over. A
            mapped column absent from the file is ``None``, which the digest
            encodes as the empty string.
    """

    source_system: str
    source_record_id: str
    payload: Mapping[str, str | None]
    values: tuple[str | None, ...]


@runtime_checkable
class SourceAdapter(Protocol):
    """The S4.1 interface every delivery mechanism implements.

    v1 ships the two drop-folder adapters below; the Protocol exists so a database
    adapter can be added later without ER-031's landing code changing.
    """

    #: The ``sources.<name>`` key this adapter reads.
    source: str
    #: Source column names in ``sources.<name>.columns`` declaration order, which
    #: is the order :attr:`SourceRow.values` follows and the ``columns`` argument
    #: of ``er.ingest.hashing.content_hash``.
    columns: tuple[str, ...]

    def discover(self) -> tuple[Path, ...]:
        """The delivery's files, in the deterministic order :meth:`rows` reads them."""

    def rows(self) -> Iterator[SourceRow]:
        """Stream the delivery's rows in file order, then in-file order."""


def discover_files(
    drop_dir: str | os.PathLike[str],
    source: str,
    extensions: Collection[str] | None = None,
) -> tuple[Path, ...]:
    """Return the files of ``<drop_dir>/<source>/``, sorted by filename.

    Args:
        drop_dir: the drop-folder root, ``storage.drop_dir`` (S6) unless ``er
            ingest --path`` overrides it.
        source: the ``sources.<name>`` key, which is also the subdirectory name.
        extensions: the suffixes to accept, lowercased; every extension any
            adapter in this module handles when omitted.

    A missing or empty directory returns no files rather than raising: S4.1.1
    makes an empty delivery a decision for the *stage* -- exit ``10`` normally,
    exit ``2`` under ``--full-refresh-keys`` -- and an adapter that raised would
    take that decision away from it.
    """
    allowed = tuple(extensions) if extensions is not None else CSV_EXTENSIONS + PARQUET_EXTENSIONS
    directory = Path(drop_dir) / source
    if not directory.is_dir():
        return ()
    matched = [
        entry
        for entry in directory.iterdir()
        if entry.is_file() and entry.suffix.lower() in allowed
    ]
    # By name, not by `iterdir` order, which is filesystem order and differs
    # between the CI runner and a developer's machine.
    return tuple(sorted(matched, key=lambda entry: entry.name))


def _render(value: object, date_format: str) -> str | None:
    """Render one delivered value to the text both delivery formats agree on.

    A CSV delivery carries text already, so a ``str`` passes through untouched.
    The typed values only Parquet can carry are rendered so the two paths hash
    identically: a date through the source's ``date_format`` (S6), a timestamp
    through ISO-8601, anything else through ``str``. NULL stays ``None``, which
    ``content_hash`` encodes as the empty string.
    """
    if value is None:
        return None
    if isinstance(value, str):
        return value
    # `datetime` is a subclass of `date`, so its arm has to come first -- the
    # other order would render every timestamp through `date_format` and silently
    # drop the time.
    if isinstance(value, dt.datetime):
        return value.isoformat()
    if isinstance(value, dt.date):
        return value.strftime(date_format)
    return str(value)


class _DropFolderAdapter:
    """Shared discovery, projection and error mapping for the two v1 adapters."""

    extensions: ClassVar[tuple[str, ...]] = ()

    def __init__(self, source: str, spec: SourceSpec, drop_dir: str | os.PathLike[str]) -> None:
        self.source = source
        self.spec = spec
        self.drop_dir = Path(drop_dir)
        # Declaration order, never sorted: S4.1 defines `content_hash` over the
        # source columns "in the declared order", and Python preserves the YAML
        # mapping order through Pydantic.
        self.columns: tuple[str, ...] = tuple(spec.columns.values())

    def discover(self) -> tuple[Path, ...]:
        return discover_files(self.drop_dir, self.source, type(self).extensions)

    def rows(self) -> Iterator[SourceRow]:
        for path in self.discover():
            yield from self._file_rows(path)

    def _file_rows(self, path: Path) -> Iterator[SourceRow]:
        raise NotImplementedError

    def _file_error(self, path: Path, cause: object) -> StageFailure:
        """S4.7 ``data``: the file cannot be read or parsed at all."""
        return StageFailure(f"{path}: unreadable source file: {cause}", error_class=ErrorClass.DATA)

    def _row_error(self, path: Path, number: int, detail: str) -> StageFailure:
        """S4.7 ``data``: one row is malformed. Names the file and the row."""
        return StageFailure(f"{path}: row {number}: {detail}", error_class=ErrorClass.DATA)

    def _require_record_id_column(self, path: Path, header: Sequence[str]) -> None:
        """Refuse a file whose header cannot supply the key, before any row is yielded."""
        if self.spec.record_id_column not in header:
            raise StageFailure(
                f"{path}: header does not carry record_id_column "
                f"{self.spec.record_id_column!r}; delivered columns are {list(header)}",
                error_class=ErrorClass.DATA,
            )

    def _row(self, raw: Mapping[str, object], path: Path, number: int) -> SourceRow:
        """Project one raw row into a :class:`SourceRow`, or raise the S4.7 ``data`` class."""
        payload = {name: _render(value, self.spec.date_format) for name, value in raw.items()}
        source_record_id = payload.get(self.spec.record_id_column)
        if source_record_id is None:
            raise self._row_error(path, number, f"{self.spec.record_id_column!r} is NULL")
        try:
            # For the S5.0 key model only; the key itself is derived where it is
            # written, so this module never carries a second spelling of it.
            record_key(self.source, source_record_id)
        except InvalidRecordKeyError as exc:
            raise self._row_error(path, number, str(exc)) from exc
        return SourceRow(
            source_system=self.source,
            source_record_id=source_record_id,
            payload=payload,
            values=tuple(payload.get(column) for column in self.columns),
        )


class CsvAdapter(_DropFolderAdapter):
    """``sources.<name>.adapter: csv`` -- a delivery of CSV files (S4.1)."""

    extensions: ClassVar[tuple[str, ...]] = CSV_EXTENSIONS

    def _file_rows(self, path: Path) -> Iterator[SourceRow]:
        try:
            handle = path.open(newline="", encoding="utf-8")
        except OSError as exc:
            raise self._file_error(path, exc) from exc
        with handle:
            reader = csv.reader(handle)
            try:
                header = next(reader, None)
            except (csv.Error, UnicodeDecodeError) as exc:
                raise self._file_error(path, exc) from exc
            if header is None:
                # A zero-byte file is an empty delivery, not a defect (S4.1.1).
                return
            self._require_record_id_column(path, header)
            number = 0
            while True:
                try:
                    raw = next(reader)
                except StopIteration:
                    return
                # Decoding is incremental, so a file whose tail is unparsable
                # still yields every row before it -- which is the laziness
                # property, and why this is caught here and not around the loop.
                except (csv.Error, UnicodeDecodeError) as exc:
                    raise self._file_error(path, exc) from exc
                if not raw:
                    # A blank line, including the one many writers leave at EOF.
                    # It is not a row, so it does not consume a row number.
                    continue
                number += 1
                if len(raw) != len(header):
                    # Zipping a ragged row against the header would silently
                    # misalign every value after the missing field, and the
                    # content hash of the misalignment would look like a real
                    # correction.
                    raise self._row_error(
                        path, number, f"expected {len(header)} fields, found {len(raw)}"
                    )
                yield self._row(dict(zip(header, raw, strict=True)), path, number)


class ParquetAdapter(_DropFolderAdapter):
    """``sources.<name>.adapter: parquet`` -- a delivery of Parquet files (S4.1).

    Typed columns are rendered by :func:`_render`, which is what makes a Parquet
    delivery project -- and therefore hash -- identically to the same records
    delivered as CSV.
    """

    extensions: ClassVar[tuple[str, ...]] = PARQUET_EXTENSIONS

    def _file_rows(self, path: Path) -> Iterator[SourceRow]:
        connection = duckdb.connect()
        try:
            try:
                result = connection.execute("SELECT * FROM read_parquet(?)", [str(path)])
                description = result.description
            except duckdb.Error as exc:
                raise self._file_error(path, exc) from exc
            if description is None:
                return
            header = [str(column[0]) for column in description]
            self._require_record_id_column(path, header)
            number = 0
            while True:
                try:
                    batch = result.fetchmany(PARQUET_BATCH_ROWS)
                except duckdb.Error as exc:
                    raise self._file_error(path, exc) from exc
                if not batch:
                    return
                for raw in batch:
                    number += 1
                    yield self._row(dict(zip(header, raw, strict=True)), path, number)
        finally:
            # Reached on exhaustion, on a raise, and on the generator being closed
            # after a partial read -- the AC8 laziness case.
            connection.close()


#: The ``sources.<name>.adapter`` tokens v1 accepts. An unknown token is a config
#: error (S4.7), so this mapping is the whole of that vocabulary.
_ADAPTERS: Mapping[str, type[_DropFolderAdapter]] = MappingProxyType(
    {"csv": CsvAdapter, "parquet": ParquetAdapter}
)


def adapter_for(
    config: Config,
    source: str,
    drop_dir: str | os.PathLike[str] | None = None,
) -> SourceAdapter:
    """Return the adapter ``sources.<source>.adapter`` names.

    Args:
        config: the validated S6 document.
        source: the ``sources.<name>`` key being ingested.
        drop_dir: the drop-folder root; ``storage.drop_dir`` when omitted, which
            is what ``er ingest --path`` overrides.

    Raises:
        ConfigError: the source or the adapter token is unknown -- S4.7's
            ``config`` class, exit ``2``, naming the offending config key. Raised
            before the drop directory is touched, so a typo cannot read a file.
    """
    spec = config.sources.get(source)
    if spec is None:
        raise ConfigError(
            f"unknown source 'sources.{source}': configured sources are {sorted(config.sources)}"
        )
    adapter_class = _ADAPTERS.get(spec.adapter)
    if adapter_class is None:
        raise ConfigError(
            f"unknown adapter 'sources.{source}.adapter' = {spec.adapter!r}: "
            f"the v1 adapters are {sorted(_ADAPTERS)}"
        )
    return adapter_class(source, spec, config.storage.drop_dir if drop_dir is None else drop_dir)
