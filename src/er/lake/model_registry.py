"""The `model_registry` lifecycle: where a trained model lives, and which one is live.

S4.3.2's "Model artifact lifecycle" is four numbered steps, and three of them are
orderings rather than operations. This module is where those orderings are expressed
once, so that no caller can express them differently:

1. **The object is written before the registry row.** A registry row is a promise that
   `params_path` resolves, and `er match` cashes it by loading the settings from S3. A
   failed upload therefore has to leave *zero* rows, which is why
   :func:`register_model` puts the object before it writes anything to the lake and
   wraps every lake write in one transaction it rolls back.
2. **`model_version` is allocated inside the insert transaction**, as zero-padded
   `max+1` — not read outside it and carried in. DuckLake enforces no `UNIQUE` (S5.0),
   so the allocation is the only thing standing between two writers and two `v0002`
   rows; the S4.0b advisory lock is what makes it sound, and this module re-derives the
   allocation inside its transaction rather than trusting the value it was handed.
3. **Activation and supersession are one transaction.** "At most one row with
   `status='active'`" is a logical key with no constraint behind it (S5.0), maintained
   by this writer and asserted by an integration test.

`corpus_snapshot` is the fourth thing here and the one worth reading carefully. S4's
idempotency table keys `train` on `(config_hash, corpus_snapshot)` and gives
`--if-changed` the job of exiting `10` when the active row already carries that key —
so the key MUST be stable across two invocations that trained over the same corpus.
The lake's *current* snapshot version is not: every `runs` row, every `run_stages`
update and every `tf_lookup` write advances it, so a training run advances it several
times on its own and no second run could ever observe the first one's value.
:func:`corpus_snapshot` therefore answers the question S5 actually asks — "the DuckLake
snapshot version the model was trained against" — as the latest snapshot in which the
corpus relation itself changed. Time travel to that version reads exactly the corpus the
model was fitted on, and a run that changes nothing reads the same number back.

This module holds no Splink and no config knowledge: it takes the settings document as
text and the prefix as a string, because `er.matching.train` is the layer that knows
what a settings document is and `er.lake` may not import it.
"""

from __future__ import annotations

import json
import re
from collections.abc import Mapping
from dataclasses import dataclass
from datetime import UTC, datetime
from typing import Any, Final, Protocol

import duckdb

from er.errors import ConfigError, PreconditionFailure, StageFailure
from er.lake.ducklake import LAKE_ALIAS
from er.lake.model import REGISTRY, SCHEMA_QUALIFIER

__all__ = [
    "ACTIVE",
    "MODEL_ARTIFACT_SUFFIX",
    "MODEL_VERSION_DIGITS",
    "MODEL_VERSION_PREFIX",
    "SUPERSEDED",
    "ModelRow",
    "ObjectWriter",
    "active_model",
    "allocate_model_version",
    "corpus_snapshot",
    "find_active_model",
    "format_model_version",
    "load_model_settings",
    "model_params_uri",
    "parse_model_version",
    "register_model",
]

#: The relation this module owns end to end (S5, `ddl.py`-owned).
MODEL_REGISTRY_RELATION: Final = "model_registry"

_REGISTRY: Final = f"{SCHEMA_QUALIFIER}.{MODEL_REGISTRY_RELATION}"

#: `model_registry.status`, spelled from S5's comment rather than from a literal at
#: each use. Both values are in :data:`er.lake.model.MODEL_STATUSES`.
ACTIVE: Final = "active"
SUPERSEDED: Final = "superseded"

#: S4.3.2's `model_v{N}` shape: a `v`, then `N` zero-padded to four digits. The width
#: is the artifact name's as well as the column's — `model_v0003.json` — so it is one
#: constant and not two.
MODEL_VERSION_PREFIX: Final = "v"
MODEL_VERSION_DIGITS: Final = 4
MODEL_ARTIFACT_SUFFIX: Final = ".json"
MODEL_ARTIFACT_STEM: Final = "model_"

#: Four digits is the pad width, not a ceiling: a lake that ever reaches `v10000`
#: keeps counting rather than wrapping, so the pattern accepts a wider number and
#: :func:`format_model_version` pads to at least the declared width.
_MODEL_VERSION_RE: Final = re.compile(
    rf"^{MODEL_VERSION_PREFIX}(?P<number>\d{{{MODEL_VERSION_DIGITS},}})$"
)

_S3_SCHEME: Final = "s3://"


class ObjectWriter(Protocol):
    """The two :class:`~er.lake.objectstore.ObjectStore` operations this module needs.

    A Protocol rather than the concrete class for the reason AC1 exists: "a failed
    upload leaves zero rows" is only assertable against a store whose put fails, and
    the failure has to come from the store rather than from a patched module global.
    """

    def put_bytes(self, uri: str, data: bytes) -> None:
        """Write ``data`` to ``uri``."""

    def get_bytes(self, uri: str) -> bytes:
        """Read ``uri`` in full."""


def format_model_version(number: int) -> str:
    """``3`` as ``'v0003'`` (S4.3.2 step 2, S5).

    Raises:
        ValueError: ``number`` is not positive. Versions are allocated as `max+1` from
            an empty registry, so the first one is ``1``; a zero or negative version
            is a caller that computed the allocation rather than asking for it.
    """
    if number < 1:
        raise ValueError(f"model_version numbers start at 1, got {number}")
    return f"{MODEL_VERSION_PREFIX}{number:0{MODEL_VERSION_DIGITS}d}"


def parse_model_version(model_version: str) -> int:
    """Inverse of :func:`format_model_version`.

    Raises:
        ValueError: ``model_version`` is not a version this module could have written.
    """
    matched = _MODEL_VERSION_RE.fullmatch(model_version)
    if matched is None:
        raise ValueError(
            f"{model_version!r} is not a model_version; the format is "
            f"{MODEL_VERSION_PREFIX}<{MODEL_VERSION_DIGITS} or more digits>, "
            f"e.g. {format_model_version(3)!r}"
        )
    return int(matched.group("number"))


def model_params_uri(model_uri_prefix: str, model_version: str) -> str:
    """`{storage.model_uri_prefix}model_v{N}.json` — the S4.3.2 step 1 artifact URI.

    The prefix is used exactly as the document supplies it. V14 already requires it to
    end in `/`, and S6 already requires it to name the tenant, so interpolating either
    here would produce `s3://lake/models/test/test/model_v0001.json` — a URI that
    resolves, that nothing rejects, and that no operator looking for the model would
    ever find (S4.3.2, D14).

    Raises:
        er.errors.ConfigError: the prefix is not an `s3://` URI ending in `/`. V14
            rejects it too, but this module is handed a string rather than a validated
            document, and a bad prefix would otherwise surface as a registry row
            pointing at an object nobody can address.
        ValueError: ``model_version`` is not a well-formed version.
    """
    if not model_uri_prefix.startswith(_S3_SCHEME) or not model_uri_prefix.endswith("/"):
        raise ConfigError(
            f"storage.uri: model_uri_prefix must be an s3:// URI ending in '/', "
            f"got {model_uri_prefix!r}"
        )
    parse_model_version(model_version)
    return f"{model_uri_prefix}{MODEL_ARTIFACT_STEM}{model_version}{MODEL_ARTIFACT_SUFFIX}"


@dataclass(frozen=True)
class ModelRow:
    """One `model_registry` row, with `metrics` already parsed (S5).

    Frozen, and `metrics` is the decoded object rather than the stored text: every
    reader of this row wants the document, and decoding it at each call site is how two
    readers end up disagreeing about whether `metrics` is a string.
    """

    model_version: str
    status: str
    trained_at: datetime
    corpus_snapshot: int
    params_path: str
    tf_tables_path: str
    tf_snapshot_id: str
    config_hash: str
    metrics: Mapping[str, Any]
    run_id: str

    @property
    def is_active(self) -> bool:
        """Whether this is the row `er match` loads without `--model-version` (S4.3.2)."""
        return self.status == ACTIVE


#: The registry's column list, in S5 order, read from the one place that declares it.
_COLUMNS: Final[tuple[str, ...]] = REGISTRY[MODEL_REGISTRY_RELATION].column_names

_SELECT_SQL: Final = f"SELECT {', '.join(_COLUMNS)} FROM {_REGISTRY}"


def _row_from(values: tuple[Any, ...]) -> ModelRow:
    """One fetched tuple as a :class:`ModelRow`, positionally against :data:`_COLUMNS`."""
    fields = dict(zip(_COLUMNS, values, strict=True))
    metrics = fields["metrics"]
    return ModelRow(
        model_version=str(fields["model_version"]),
        status=str(fields["status"]),
        trained_at=fields["trained_at"],
        corpus_snapshot=int(fields["corpus_snapshot"]),
        params_path=str(fields["params_path"]),
        tf_tables_path=str(fields["tf_tables_path"]),
        tf_snapshot_id=str(fields["tf_snapshot_id"]),
        config_hash=str(fields["config_hash"]),
        # `metrics` is a `JSON` column, which DuckDB hands back as text.
        metrics=json.loads(metrics) if isinstance(metrics, str) else dict(metrics),
        run_id=str(fields["run_id"]),
    )


def allocate_model_version(connection: duckdb.DuckDBPyConnection) -> str:
    """The next `model_version`: zero-padded `max+1` over the live registry (S4.3.2).

    Called by :func:`register_model` **inside** its transaction, which is where S4.3.2
    puts the allocation. A caller that needs the value earlier — `er train` freezes
    `tf_lookup` under it before it fits anything — may call it too, and hands the value
    back for :func:`register_model` to re-derive and check.

    The maximum is taken over the parsed numbers rather than over the `VARCHAR`. The
    two agree for as long as every version is the same width, and stop agreeing at
    `v10000`, where `'v9999' > 'v10000'` lexically and the allocator would hand out
    `v10000` twice.
    """
    numbers = [
        parse_model_version(str(version))
        for (version,) in connection.execute(f"SELECT model_version FROM {_REGISTRY}").fetchall()
    ]
    return format_model_version(max(numbers, default=0) + 1)


def find_active_model(connection: duckdb.DuckDBPyConnection) -> ModelRow | None:
    """The `status='active'` row, or ``None`` when the registry holds none.

    For the callers to which "no model yet" is an ordinary state — `er train
    --if-changed` on a fresh lake has nothing to compare against and must train, not
    refuse. Every other caller wants :func:`active_model`.

    Raises:
        er.errors.StageFailure: more than one row is active. DuckLake enforces no
            filtered uniqueness (S5.0), so the invariant is this module's to keep, and
            silently picking one of two would make `er match` non-deterministic.
    """
    rows = connection.execute(f"{_SELECT_SQL} WHERE status = ?", [ACTIVE]).fetchall()
    if not rows:
        return None
    if len(rows) > 1:
        versions = sorted(str(row[0]) for row in rows)
        raise StageFailure(
            f"{MODEL_REGISTRY_RELATION} holds {len(rows)} rows with status={ACTIVE!r} "
            f"({', '.join(versions)}); S5.0 permits at most one, and DuckLake enforces "
            f"no filtered uniqueness, so this is a writer that did not supersede"
        )
    return _row_from(rows[0])


def active_model(connection: duckdb.DuckDBPyConnection) -> ModelRow:
    """The `status='active'` row `er match` loads by default (S4.3.2 step 4).

    Raises:
        er.errors.PreconditionFailure: the registry holds no active row. S4.0 gives
            "no `status='active'` model" exit ``3``: the stage has not begun, and the
            operator action is `er train`, not a retry.
        er.errors.StageFailure: more than one row is active (see
            :func:`find_active_model`).
    """
    row = find_active_model(connection)
    if row is None:
        raise PreconditionFailure(
            f"no {MODEL_REGISTRY_RELATION} row has status={ACTIVE!r}; run `er train` "
            f"before a stage that scores (S4.0 exit 3, S4.3.2)"
        )
    return row


def load_model_settings(
    connection: duckdb.DuckDBPyConnection, store: ObjectWriter, model_version: str
) -> dict[str, Any]:
    """The settings document `model_version` was registered with, read back from S3.

    The registry row is consulted for `params_path` rather than the URI being rebuilt
    from the prefix: the row is what S4.3.2 makes authoritative, and a document loaded
    from a recomputed path would silently follow a `model_uri_prefix` that had changed
    since the model was trained.

    Raises:
        er.errors.PreconditionFailure: no row carries this `model_version`.
        er.errors.StageFailure: the object is not a JSON object.
    """
    rows = connection.execute(
        f"SELECT params_path FROM {_REGISTRY} WHERE model_version = ?", [model_version]
    ).fetchall()
    if not rows:
        raise PreconditionFailure(
            f"{MODEL_REGISTRY_RELATION} holds no row for model_version={model_version!r}"
        )
    params_path = str(rows[0][0])
    document = json.loads(store.get_bytes(params_path).decode("utf-8"))
    if not isinstance(document, dict):
        raise StageFailure(
            f"{params_path} holds a {type(document).__name__}, not a settings document; "
            f"the {MODEL_REGISTRY_RELATION} row for {model_version!r} points at an "
            f"object that is not a model"
        )
    return document


#: Every snapshot in the lake's log, with the change record flattened to the bare list
#: of things it names. DuckLake spells a change as a MAP whose keys vary by operation
#: (`tables_created`, `tables_inserted_into`, `tables_deleted_from`, `tables_dropped`,
#: the inlined variants) and whose values are either table ids or qualified names — so
#: matching on the flattened VALUES asks "did this snapshot touch this table?" without
#: depending on the key vocabulary, which is a DuckLake implementation detail and not a
#: contract we may pin.
_CORPUS_SNAPSHOT_SQL: Final = f"""
SELECT max(snapshot.snapshot_id)
  FROM {LAKE_ALIAS}.snapshots() AS snapshot
 WHERE list_contains(flatten(map_values(snapshot.changes)), ?)
    OR list_contains(flatten(map_values(snapshot.changes)), ?)
"""

_TABLE_ID_SQL: Final = f"SELECT table_id FROM {LAKE_ALIAS}.table_info() WHERE table_name = ?"

#: The schema every relation of S5 lives in, as the snapshot log spells a table name.
_SCHEMA: Final = SCHEMA_QUALIFIER.split(".")[-1]


def corpus_snapshot(connection: duckdb.DuckDBPyConnection, relation: str) -> int:
    """The latest snapshot version in which ``relation`` changed (S4.3.2, S5).

    This is what `model_registry.corpus_snapshot` records and what the S4 idempotency
    key `(config_hash, corpus_snapshot)` is half of. It is deliberately NOT
    :func:`er.lake.ducklake.current_snapshot`: that number advances on every write the
    lake takes, including the `runs`, `run_stages` and `tf_lookup` writes a training
    run makes for itself, so an `er train --if-changed` could never observe the value
    the previous run recorded and the S4 row would describe a branch nothing reaches.

    Read from the snapshot log rather than tracked by us, so it stays true for a corpus
    rebuilt by dbt, by `er standardize` or by a fixture — none of which are in a
    position to tell the registry what they did.

    Args:
        connection: a connection with the lake attached, per S4.0b.
        relation: the bare corpus relation name, e.g. `int_std_records`.

    Returns:
        The snapshot version. ``0`` when the relation has never been written, which is
        the honest answer for a lake whose corpus does not exist yet — and which
        `er train` will fail on for the better reason that there is nothing to train
        over.
    """
    ids = connection.execute(_TABLE_ID_SQL, [relation]).fetchall()
    # The id is matched as well as the qualified name because the two appear under
    # different keys: DuckLake names a table on creation and identifies it by id
    # thereafter, and a `CREATE OR REPLACE` mints a NEW id — so the name is what links
    # the current relation to the snapshot that replaced it.
    table_id = "" if not ids else str(ids[0][0])
    row = connection.execute(_CORPUS_SNAPSHOT_SQL, [table_id, f"{_SCHEMA}.{relation}"]).fetchone()
    if row is None or row[0] is None:
        return 0
    return int(row[0])


_INSERT_SQL: Final = (
    f"INSERT INTO {_REGISTRY} ({', '.join(_COLUMNS)}) VALUES ({', '.join('?' for _ in _COLUMNS)})"
)

_SUPERSEDE_SQL: Final = f"UPDATE {_REGISTRY} SET status = ? WHERE status = ?"


def register_model(
    connection: duckdb.DuckDBPyConnection,
    store: ObjectWriter,
    *,
    model_version: str,
    model_uri_prefix: str,
    settings_json: str,
    metrics: Mapping[str, Any],
    corpus_snapshot: int,
    tf_snapshot_id: str,
    tf_tables_path: str,
    config_hash: str,
    run_id: str,
    trained_at: datetime | None = None,
) -> ModelRow:
    """Write the artifact, then activate it as a new `model_registry` row (S4.3.2).

    The order of operations is the specification, so it is worth stating as code
    reads it:

    1. `BEGIN` — everything the lake sees from here is one commit or none.
    2. Allocate `max+1` **inside** that transaction and check it against
       ``model_version``, which is the version the caller already froze `tf_lookup`
       under. S4.0b gives one writer per tenant, so the two agree; a disagreement means
       a second writer allocated between the two calls, and the frozen TF rows belong
       to a version this row would not name.
    3. `put_bytes` — **before** any registry write. A registry row that points at a
       missing object is the failure this ordering exists to make unreachable, and the
       upload is the only step that can fail for reasons outside the lake.
    4. Supersede the previous active row, then insert this one as `active`. One
       transaction, so "at most one active row" is never observably false.

    Args:
        connection: a connection with the lake attached, per S4.0b.
        store: where the artifact goes; `er train` passes an
            :class:`~er.lake.objectstore.ObjectStore`.
        model_version: the version the caller allocated and froze TF under.
        model_uri_prefix: `storage.model_uri_prefix`, verbatim from S6.
        settings_json: the fitted settings document, already serialised. Bytes are what
            reach S3, and re-serialising here would let this module's JSON options
            disagree with the ones the artifact was compared under.
        metrics: the `metrics` payload — S4.3.2 requires the verbatim `training:` block
            and the fitted values.
        corpus_snapshot: from :func:`corpus_snapshot`, captured before training began.
        tf_snapshot_id, tf_tables_path: the frozen TF key and its locator (S4.3.3).
        config_hash: the run's `config_hash` (S5.2).
        run_id: the run that trained the model.
        trained_at: the timestamp for the row; now, in UTC, when omitted.

    Returns:
        The row as written, with `params_path` resolved.

    Raises:
        er.errors.StageFailure: the allocation moved under the caller.
        Exception: whatever ``store`` raises. It is re-raised after the transaction is
            rolled back, so the registry is exactly as it was found.
    """
    params_path = model_params_uri(model_uri_prefix, model_version)
    stamped = datetime.now(UTC) if trained_at is None else trained_at
    # S5's timestamp columns are `TIMESTAMP`, not `TIMESTAMPTZ` (S5.0), so the offset
    # is dropped at this boundary rather than left to an implicit engine cast.
    naive = stamped.astimezone(UTC).replace(tzinfo=None)

    connection.execute("BEGIN TRANSACTION")
    try:
        allocated = allocate_model_version(connection)
        if allocated != model_version:
            raise StageFailure(
                f"model_version allocation moved: the corpus was frozen under "
                f"{model_version!r} and the registry now allocates {allocated!r}. "
                f"S4.0b gives one writer per tenant, so this is a second writer in "
                f"this namespace and the frozen tf_lookup rows belong to neither row"
            )
        # Step 1 of S4.3.2, and it precedes every lake write below it: a failed upload
        # leaves the registry untouched, and the rollback below leaves nothing behind.
        store.put_bytes(params_path, settings_json.encode("utf-8"))
        connection.execute(_SUPERSEDE_SQL, [SUPERSEDED, ACTIVE])
        values: dict[str, Any] = {
            "model_version": model_version,
            "status": ACTIVE,
            "trained_at": naive,
            "corpus_snapshot": corpus_snapshot,
            "params_path": params_path,
            "tf_tables_path": tf_tables_path,
            "tf_snapshot_id": tf_snapshot_id,
            "config_hash": config_hash,
            "metrics": json.dumps(dict(metrics), sort_keys=True, ensure_ascii=False),
            "run_id": run_id,
        }
        connection.execute(_INSERT_SQL, [values[column] for column in _COLUMNS])
    except BaseException:
        connection.execute("ROLLBACK")
        raise
    connection.execute("COMMIT")

    return ModelRow(
        model_version=model_version,
        status=ACTIVE,
        trained_at=naive,
        corpus_snapshot=corpus_snapshot,
        params_path=params_path,
        tf_tables_path=tf_tables_path,
        tf_snapshot_id=tf_snapshot_id,
        config_hash=config_hash,
        metrics=dict(metrics),
        run_id=run_id,
    )
