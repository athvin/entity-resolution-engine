"""The S4.1 ingest write: anti-join append to `raw_records` (DesignDoc.md S4.1, S5).

This is the first stage that puts real rows in the lake, so it is also where the S4
preamble's idempotency contract stops being a claim and becomes executable: the
stage's key is `(source_system, source_record_id, content_hash)`, the write is an
**anti-join append** against that key, and a re-delivered identical file therefore
appends zero rows and exits `10`.

Four rules are structural here, and each is a decision the spec makes:

* **Append, never upsert.** D7 makes `raw_records` version history: a corrected
  record ADDS a row and never overwrites one. Nothing in this module issues an
  `UPDATE`, a `DELETE` or a `MERGE` against `raw_records`, and the whole write is a
  single statement (:func:`anti_join_append`) so a delivery cannot be half-applied.
* **One `content_hash`, one implementation.** S4.1 spells the function
  ``er.ingest.landing.content_hash`` while S3 puts the module at
  ``ingest/hashing.py``. :data:`content_hash` here is ER-029's function *bound*, not
  wrapped and not reimplemented — ``er.ingest.landing.content_hash is
  er.ingest.hashing.content_hash`` — because two implementations that disagree make
  T-IDEM-1a and T-IDEM-1 non-reproducible.
* **Classify before you append.** The five counts are read off the lake as it stood
  *before* the append; computing them afterwards would classify every delivered key
  as `unchanged`, which is exactly the reading that would make a broken anti-join
  look idempotent.
* **The manifest row is written on every accepted invocation**, including the no-op
  one that exits `10`. T-IDEM-1a asserts on the *second* `ingest_batches` row's
  counts, so a stage that skipped the row when it appended nothing would leave that
  assertion with nothing to read.

The five counts are the five `ingest_batches` columns of S5, the five S4.1.1 stage
counters, and the five short labels on the S4.0 stdout manifest line — the same five
numbers under three spellings, and :class:`IngestManifest` is the single object all
three are rendered from so no sixth counter can appear.

Out of scope by construction and owned by ER-032: tombstones, ``--full-refresh-keys``,
the S4.1.1 empty-delivery guard and resurrection. This stage writes
``tombstone_count = 0``, ``resurrected_count = 0`` and ``full_refresh_keys = false``
because those are the true values of the only arm it implements.
"""

from __future__ import annotations

import json
from collections.abc import Iterable, Iterator, Sequence
from dataclasses import dataclass
from datetime import UTC, datetime
from pathlib import Path
from typing import Final

import duckdb

from er.config.schema import Config
from er.entities.ids import IdFactory, UlidFactory
from er.errors import ExitCode
from er.ingest.hashing import content_hash
from er.ingest.sources import SourceAdapter, SourceRow, adapter_for
from er.lake.model import REGISTRY, SCHEMA_QUALIFIER
from er.obs.runctx import StageRun

__all__ = [
    "ANTI_JOIN_KEY",
    "APPENDED_IS_DELETED",
    "STAGING_TABLE",
    "AppendResult",
    "IngestManifest",
    "anti_join_append",
    "content_hash",
    "ingest_delivery",
    "write_ingest_batch",
]

#: The logical key S4.1 makes the ingest idempotency key and S5.0 makes
#: `raw_records`'s logical key. Read off the registry rather than transcribed: the
#: anti-join predicate and the relation's declared key are the same three columns,
#: and a second spelling here would be the one place they could drift apart.
ANTI_JOIN_KEY: Final[tuple[str, ...]] = REGISTRY["raw_records"].keys[0].columns

#: Every row this stage appends is a live content version. The tombstone arm that
#: writes ``true`` — with the S4.1.1 sentinel hash and a NULL payload — is ER-032.
APPENDED_IS_DELETED: Final[bool] = False

#: Where the delivery lands before the single write statement reads it. A TEMP
#: relation, so it lives in the ``:memory:`` primary database and never commits a
#: DuckLake snapshot (S4.0b): staging into the lake would make the intermediate
#: state of a delivery time-travellable, and a failed append would leave it there.
STAGING_TABLE: Final[str] = "er_ingest_delivery"

#: Rows handed to DuckDB per `executemany`. The adapter streams (S4.1), and this is
#: what keeps that property true through the staging load: a delivery at S10.2 scale
#: is never resident, in this module any more than in ``sources.py``.
STAGE_BATCH_ROWS: Final[int] = 1024

_RAW_RECORDS: Final[str] = "raw_records"
_INGEST_BATCHES: Final[str] = "ingest_batches"

_CREATE_STAGING: Final[str] = f"""
CREATE OR REPLACE TEMP TABLE {STAGING_TABLE} (
    seq              BIGINT  NOT NULL,
    source_system    VARCHAR NOT NULL,
    source_record_id VARCHAR NOT NULL,
    payload          VARCHAR NOT NULL,
    content_hash     VARCHAR NOT NULL
)
"""

_INSERT_STAGING: Final[str] = f"INSERT INTO {STAGING_TABLE} VALUES (?, ?, ?, ?, ?)"

# The S4.1 classification, evaluated against the lake as it stands BEFORE the append.
#
# It is per delivered KEY, not per delivered row: S4.1 defines the three counts over
# keys ("keys not previously seen", "known key with a new content_hash"), so they sum
# to the number of distinct keys the delivery carries however many versions of a key
# it happens to hold. A key delivered twice, once at a known hash and once at a new
# one, is `changed` — `bool_and` is what makes "every delivered version is already
# here" the condition for `unchanged`.
_CLASSIFY: Final[str] = f"""
WITH delivered AS (
    SELECT DISTINCT source_system, source_record_id, content_hash
    FROM {STAGING_TABLE}
),
known_keys AS (
    SELECT DISTINCT source_system, source_record_id
    FROM {SCHEMA_QUALIFIER}.{_RAW_RECORDS}
    WHERE source_system = ?
),
known_versions AS (
    SELECT DISTINCT source_system, source_record_id, content_hash
    FROM {SCHEMA_QUALIFIER}.{_RAW_RECORDS}
    WHERE source_system = ?
),
classified AS (
    SELECT
        d.source_system,
        d.source_record_id,
        k.source_record_id IS NOT NULL AS key_known,
        v.content_hash IS NOT NULL AS version_known
    FROM delivered AS d
    LEFT JOIN known_keys AS k
        ON k.source_system = d.source_system
       AND k.source_record_id = d.source_record_id
    LEFT JOIN known_versions AS v
        ON v.source_system = d.source_system
       AND v.source_record_id = d.source_record_id
       AND v.content_hash = d.content_hash
),
per_key AS (
    SELECT
        source_system,
        source_record_id,
        bool_and(key_known) AS key_known,
        bool_and(version_known) AS version_known
    FROM classified
    GROUP BY source_system, source_record_id
)
SELECT
    count(*) FILTER (WHERE NOT key_known) AS new_count,
    count(*) FILTER (WHERE key_known AND NOT version_known) AS changed_count,
    count(*) FILTER (WHERE key_known AND version_known) AS unchanged_count
FROM per_key
"""

# THE write. One statement, an anti-join against the S4.1 key, and no `UPDATE`, no
# `DELETE` and no `MERGE` anywhere near `raw_records` (D7).
#
# `arg_min(payload, seq)` collapses a (key, hash) pair delivered more than once in one
# file: the anti-join alone would append one row per duplicate, because the row it is
# anti-joining against does not exist yet. `seq` makes the survivor the first-delivered
# one rather than an arbitrary one, so two runs over the same delivery store the same
# payload bytes.
_APPEND: Final[str] = f"""
INSERT INTO {SCHEMA_QUALIFIER}.{_RAW_RECORDS}
    (source_system, source_record_id, payload, content_hash,
     is_deleted, deleted_at, ingest_batch_id, ingested_at)
SELECT
    d.source_system,
    d.source_record_id,
    CAST(d.payload AS JSON),
    d.content_hash,
    ?,
    CAST(NULL AS TIMESTAMP),
    ?,
    ?
FROM (
    SELECT
        source_system,
        source_record_id,
        content_hash,
        arg_min(payload, seq) AS payload
    FROM {STAGING_TABLE}
    GROUP BY source_system, source_record_id, content_hash
) AS d
WHERE NOT EXISTS (
    SELECT 1
    FROM {SCHEMA_QUALIFIER}.{_RAW_RECORDS} AS r
    WHERE r.source_system = d.source_system
      AND r.source_record_id = d.source_record_id
      AND r.content_hash = d.content_hash
)
"""


def _now() -> datetime:
    """The naive UTC instant a `TIMESTAMP` column takes (S5.0: no `TIMESTAMPTZ`)."""
    return datetime.now(UTC).replace(tzinfo=None)


@dataclass(frozen=True)
class AppendResult:
    """What one :func:`anti_join_append` did, in S4.1's own vocabulary.

    Attributes:
        rows_in: source rows read from the delivery, duplicates included. The
            `run_stages.rows_in` of S4.1.1.
        rows_out: `raw_records` rows appended. The `run_stages.rows_out` of S4.1.1,
            and zero for a re-delivered identical file.
        new_count: delivered keys not previously present in `raw_records`.
        changed_count: known keys carrying at least one unseen `content_hash`.
        unchanged_count: known keys every one of whose delivered versions was
            already present — counted, and deliberately not appended.
    """

    rows_in: int
    rows_out: int
    new_count: int
    changed_count: int
    unchanged_count: int


@dataclass(frozen=True)
class IngestManifest:
    """One `ingest_batches` row, and the three spellings S4.1 renders it in.

    The relation's column list is S5's and is not restated here as a second
    declaration: :meth:`row` is checked against the registry by
    :func:`write_ingest_batch`, so a column added to S5 is a failure at write time
    rather than a silently missing value.
    """

    ingest_batch_id: str
    run_id: str
    source_system: str
    path: str
    new_count: int
    changed_count: int
    unchanged_count: int
    tombstone_count: int
    resurrected_count: int
    full_refresh_keys: bool
    created_at: datetime
    files: int
    rows_in: int
    rows_out: int

    @property
    def exit_code(self) -> int:
        """S4.0: `10` when ``new = 0 and changed = 0 and tombstoned = 0``.

        A no-op, not a failure — it never aborts an `er run-all` chain, and the
        `ingest_batches` row is written either way.
        """
        if self.new_count == 0 and self.changed_count == 0 and self.tombstone_count == 0:
            return int(ExitCode.NOTHING_TO_DO)
        return int(ExitCode.SUCCESS)

    def row(self) -> dict[str, object]:
        """The `ingest_batches` row, keyed by S5's column names."""
        return {
            "ingest_batch_id": self.ingest_batch_id,
            "run_id": self.run_id,
            "source_system": self.source_system,
            "path": self.path,
            "new_count": self.new_count,
            "changed_count": self.changed_count,
            "unchanged_count": self.unchanged_count,
            "tombstone_count": self.tombstone_count,
            "resurrected_count": self.resurrected_count,
            "full_refresh_keys": self.full_refresh_keys,
            "created_at": self.created_at,
        }

    def manifest(self) -> dict[str, object]:
        """The S4.0 stdout manifest line's fields, under their short labels.

        `new`, `changed`, `unchanged`, `tombstoned` and `resurrected` are the same
        five numbers the `ingest_batches` columns and the S4.1.1 counters carry —
        S4.1 is explicit that there is no sixth counter, so all three renderings are
        derived from this one object.
        """
        return {
            "source": self.source_system,
            "files": self.files,
            "new": self.new_count,
            "changed": self.changed_count,
            "unchanged": self.unchanged_count,
            "tombstoned": self.tombstone_count,
            "resurrected": self.resurrected_count,
            "ingest_batch_id": self.ingest_batch_id,
        }

    def stdout_line(self) -> str:
        """:meth:`manifest` as the human line S4.0's stdout column describes."""
        return ", ".join(f"{name}={value}" for name, value in self.manifest().items())

    def counters(self) -> dict[str, object]:
        """The S4.1.1 counter payload: the five counts under their long names.

        `rows_in` and `rows_out` are here too because S5.2's completeness rule makes
        the payload the union of the S4 list and the promoted counters the stage
        wrote; :meth:`record` routes each name to the column S5.2 promotes it to.
        """
        return {
            "files": self.files,
            "new_count": self.new_count,
            "changed_count": self.changed_count,
            "unchanged_count": self.unchanged_count,
            "tombstone_count": self.tombstone_count,
            "resurrected_count": self.resurrected_count,
            "ingest_batch_id": self.ingest_batch_id,
            "rows_in": self.rows_in,
            "rows_out": self.rows_out,
        }

    def record(self, stage_run: StageRun) -> None:
        """Write :meth:`counters` onto the stage's `run_stages` row (S4.1.1, S5.2).

        Through :meth:`~er.obs.counters.StageCounters.set` for every name, so
        `rows_in` and `rows_out` reach their typed columns and the other seven reach
        the JSON payload without this method having to know which list each is on.
        `duration_ms` is not written here: it is the one promoted counter the stage
        cannot measure for itself, and the run stamps it on the way out.
        """
        for name, value in self.counters().items():
            stage_run.counters.set(name, value)


def _hashed_rows(
    rows: Iterable[SourceRow], columns: Sequence[str]
) -> Iterator[tuple[int, str, str, str, str]]:
    """Stream the delivery as staging tuples, hashing each row on the way past.

    Lazy, because :meth:`~er.ingest.sources.SourceAdapter.rows` is: a delivery at
    S10.2 scale must never be resident, and materializing it here would undo the
    property ER-030 went to the trouble of establishing.

    `payload` is the full source row verbatim as JSON (S4.1) — every delivered
    column, including the ones ``sources.<name>.columns`` does not name, because the
    S4.2 staging models extract from it.
    """
    for seq, row in enumerate(rows, start=1):
        payload = json.dumps(dict(row.payload), separators=(",", ":"), ensure_ascii=False)
        yield (
            seq,
            row.source_system,
            row.source_record_id,
            payload,
            content_hash(row.payload, columns),
        )


def _stage_delivery(
    connection: duckdb.DuckDBPyConnection,
    rows: Iterable[SourceRow],
    columns: Sequence[str],
) -> int:
    """Load the delivery into :data:`STAGING_TABLE`; return the rows read."""
    connection.execute(_CREATE_STAGING)
    rows_in = 0
    batch: list[tuple[int, str, str, str, str]] = []
    for staged in _hashed_rows(rows, columns):
        batch.append(staged)
        if len(batch) >= STAGE_BATCH_ROWS:
            connection.executemany(_INSERT_STAGING, batch)
            rows_in += len(batch)
            batch.clear()
    if batch:
        connection.executemany(_INSERT_STAGING, batch)
        rows_in += len(batch)
    return rows_in


def _classify(connection: duckdb.DuckDBPyConnection, source: str) -> tuple[int, int, int]:
    """The three S4.1 counts, against the lake as it stands before the append."""
    row = connection.execute(_CLASSIFY, [source, source]).fetchone()
    if row is None:
        # `count(*)` over an empty grouping still returns one row, so this is
        # unreachable; stated rather than asserted so an engine that ever returned
        # nothing produces zeroes instead of a `TypeError` two frames away.
        return (0, 0, 0)
    return (int(row[0]), int(row[1]), int(row[2]))


def anti_join_append(
    connection: duckdb.DuckDBPyConnection,
    source: str,
    rows: Iterable[SourceRow],
    columns: Sequence[str],
    *,
    ingest_batch_id: str,
    ingested_at: datetime,
) -> AppendResult:
    """Append ``rows`` to `raw_records` as an anti-join on the S4.1 key.

    The three steps are ordered and the order is the contract: stage the delivery,
    classify it against the lake **as it stands**, then append. Classifying after the
    append would report every delivered key as `unchanged` and would make a write
    that appended nothing indistinguishable from one that appended everything.

    Args:
        connection: an attached lake connection (S4.0b).
        source: the `sources.<name>` key being ingested, which is also every staged
            row's `source_system` and the predicate the classification prunes on.
        rows: the delivery, streamed.
        columns: the source column names of ``sources.<source>.columns`` **in the
            declared order** — the input tuple :func:`content_hash` is defined over.
        ingest_batch_id: the batch's ULID, stamped on every appended row.
        ingested_at: the batch's instant, stamped on every appended row.

    Returns:
        The rows read, the rows appended, and the three S4.1 counts.
    """
    try:
        rows_in = _stage_delivery(connection, rows, columns)
        new_count, changed_count, unchanged_count = _classify(connection, source)
        appended = connection.execute(
            _APPEND, [APPENDED_IS_DELETED, ingest_batch_id, ingested_at]
        ).fetchone()
        rows_out = 0 if appended is None else int(appended[0])
    finally:
        # The staging relation is scratch, and a connection is reused across stages
        # in-process (S8.1's harness does exactly that). Leaving it behind would let
        # one delivery's rows be classified against the next one's.
        connection.execute(f"DROP TABLE IF EXISTS {STAGING_TABLE}")
    return AppendResult(
        rows_in=rows_in,
        rows_out=rows_out,
        new_count=new_count,
        changed_count=changed_count,
        unchanged_count=unchanged_count,
    )


def write_ingest_batch(connection: duckdb.DuckDBPyConnection, manifest: IngestManifest) -> None:
    """Persist ``manifest`` as one `ingest_batches` row (S4.1, S5, S5.2).

    Written on every accepted invocation, including the no-op one that exits `10`:
    T-IDEM-1a asserts `new_count = 0 AND changed_count = 0 AND tombstone_count = 0`
    on the *second* row, which only exists if the no-op wrote one.

    Raises:
        ValueError: the manifest does not name every column S5 declares. DuckLake has
            no `DEFAULT` (B2), so a partial row is a caller that has drifted from the
            registry rather than a row with holes in it.
    """
    columns = REGISTRY[_INGEST_BATCHES].column_names
    values = manifest.row()
    if set(values) != set(columns):
        raise ValueError(
            f"{_INGEST_BATCHES}: row does not match the registry; "
            f"missing={sorted(set(columns) - set(values))}, "
            f"extra={sorted(set(values) - set(columns))}"
        )
    placeholders = ",".join("?" for _ in columns)
    connection.execute(
        f"INSERT INTO {SCHEMA_QUALIFIER}.{_INGEST_BATCHES} ({','.join(columns)}) "
        f"VALUES ({placeholders})",
        [values[column] for column in columns],
    )


def ingest_delivery(
    cfg: Config,
    source: str,
    path: str | Path,
    conn: duckdb.DuckDBPyConnection,
    run_ctx: StageRun,
    *,
    id_factory: IdFactory | None = None,
) -> IngestManifest:
    """Run the S4.1 ingest stage over one delivery: read, hash, append, manifest.

    Args:
        cfg: the validated S6 document.
        source: the `sources.<name>` key being ingested.
        path: the drop-folder root the delivery lives under, as `er ingest --path`
            supplies it. The files read are `<path>/<source>/` (S4.1), so this is the
            override of `storage.drop_dir` and not the source directory itself.
        conn: an attached lake connection (S4.0b).
        run_ctx: the stage's `run_stages` row — the source of `run_id` for the
            `ingest_batches` row, and the recorder the S4.1.1 counters are written to.
        id_factory: the source of the batch ULID; production ULIDs by default.

    Returns:
        The manifest that was persisted, whose
        :attr:`~IngestManifest.exit_code` is the stage's S4.0 status.

    Raises:
        er.errors.ConfigError: `sources.<source>` is unknown (exit `2`). Raised by
            :func:`~er.ingest.sources.adapter_for` before the drop directory is
            touched, so a typo cannot read a file.
        er.errors.StageFailure: a delivered file is unreadable or unparsable, or a
            row is malformed (exit `1`, S4.7's `data` class).
    """
    adapter: SourceAdapter = adapter_for(cfg, source, path)
    factory = id_factory if id_factory is not None else UlidFactory()
    ingest_batch_id = factory.new()
    ingested_at = _now()
    files = len(adapter.discover())
    appended = anti_join_append(
        conn,
        source,
        adapter.rows(),
        adapter.columns,
        ingest_batch_id=ingest_batch_id,
        ingested_at=ingested_at,
    )
    manifest = IngestManifest(
        ingest_batch_id=ingest_batch_id,
        run_id=run_ctx.run_id,
        source_system=source,
        path=str(path),
        new_count=appended.new_count,
        changed_count=appended.changed_count,
        unchanged_count=appended.unchanged_count,
        # S4.1.1's deletion arm is ER-032; zero is the true count of what this arm
        # writes, not a placeholder for a number it failed to compute.
        tombstone_count=0,
        resurrected_count=0,
        full_refresh_keys=False,
        created_at=ingested_at,
        files=files,
        rows_in=appended.rows_in,
        rows_out=appended.rows_out,
    )
    write_ingest_batch(conn, manifest)
    manifest.record(run_ctx)
    return manifest
