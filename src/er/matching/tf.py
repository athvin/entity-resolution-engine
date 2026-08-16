"""Frozen term frequency: the D4 mechanism and the confinement it rests on (S4.3.3).

Splink computes term frequencies from whatever corpus is registered at predict time.
That is the whole problem D4 exists to solve: the same pair would score differently in
a base run, an incremental run and a full re-resolution, because each of the three
registers a different corpus — and INV-SCORE ("`match_probability` is a pure function
of `(model_version, tf_snapshot_id, rec_a_key, rec_b_key, rec_a_content_hash,
rec_b_content_hash)`") would be false. v1 therefore materializes TF once, at training
time, into `lake.main.tf_lookup` keyed by `(model_version, tf_snapshot_id)`, and every
later scoring run *registers* those frozen rows instead of computing new ones.

This module is the schema and plumbing half of that policy. It knows nothing about
`er train`, `model_registry` or scoring; it owns five things and no more:

* :func:`tf_columns` — which columns are TF columns, read from `comparisons.<attr>.tf`
  and nothing else. S4.3.1 confines `tf: true` to the exact-match level of the named
  column, so the flag is a property of the attribute, never of a level.
* :func:`new_tf_snapshot_id` — the ONE mint site. D4 permits exactly two callers:
  `er train`, and `er correct` through `er match --mode full --new-tf-snapshot`. A
  third caller would introduce a TF boundary no `match_scores` row records having
  crossed, so the confinement is asserted by a guard test rather than trusted.
* :func:`materialize_tf_lookup` — the frozen rows, computed from `int_std_records`.
* :func:`register_tf` — one `register_term_frequency_lookup` call per TF column,
  reading those rows back. It is the reason no scoring path ever needs Splink's own
  TF computation, which the same guard test asserts is called nowhere in `src/er/`.
* :func:`tf_tables_path` / :func:`parse_tf_tables_path` — the single producer/parser
  pair for the `model_registry.tf_tables_path` string, pinned as
  ``lake.main.tf_lookup?model_version=<mv>&tf_snapshot_id=<ts>``. It is a locator, not
  a URI: the rows live in the lake, and the query part names the key that selects them.

Two constraints are easy to miss. DuckLake enforces `NOT NULL` and nothing else
(S5.0), so `tf_lookup`'s logical key is unenforced by the engine and
:func:`materialize_tf_lookup` deletes the key before it writes it rather than trusting
a duplicate to be rejected. And `tf_lookup.value` is `VARCHAR`, so a TF column is cast
on the way in and read back as text — which round-trips exactly for the S4.3.1
attributes TF applies to (they are `VARCHAR` on `int_std_records`) and is why the
materializer refuses a column that relation does not have.

The failure direction is deliberate: :func:`materialize_tf_lookup` is a delete then an
insert, so a crash between them leaves the key with no rows, and :func:`register_tf`
then refuses to register at all (:class:`MissingTfLookupError`, S4.0 exit ``3``). A
scoring run that cannot find its frozen TF must stop, not fall back to computing it.
"""

from __future__ import annotations

import re
from typing import Any, Final, Protocol

import duckdb

from er.config.schema import Config
from er.entities.ids import IdFactory, MonotonicUlidFactory
from er.errors import ConfigError, PreconditionFailure
from er.lake.columns import STD_RECORD_COLUMNS
from er.lake.model import SCHEMA_QUALIFIER

__all__ = [
    "TF_COLUMN_PREFIX",
    "TF_LOOKUP_RELATION",
    "MissingTfLookupError",
    "TfLinker",
    "TfTableManagement",
    "materialize_tf_lookup",
    "new_tf_snapshot_id",
    "parse_tf_tables_path",
    "register_tf",
    "tf_columns",
    "tf_tables_path",
]

#: The relation the frozen rows live in (S5), and the corpus they are computed from.
#: Both are `lake.main.`-qualified at every use: S4.0b forbids DuckLake being the
#: default catalog, so an unqualified name here would resolve to the in-memory
#: database Splink materializes into.
TF_LOOKUP_RELATION: Final = "tf_lookup"
STD_RECORDS_RELATION: Final = "int_std_records"

_TF_LOOKUP: Final = f"{SCHEMA_QUALIFIER}.{TF_LOOKUP_RELATION}"
_STD_RECORDS: Final = f"{SCHEMA_QUALIFIER}.{STD_RECORDS_RELATION}"

#: How Splink names the frequency column of a registered TF table: the default of
#: `column_info_settings.term_frequency_adjustment_column_prefix`, which
#: :func:`er.matching.model.build_settings` does not override. A registered table is
#: read by column name — `<col>` and `tf_<col>` — so this prefix is part of the
#: interface, not a cosmetic choice.
TF_COLUMN_PREFIX: Final = "tf_"


class MissingTfLookupError(PreconditionFailure):
    """No frozen TF rows exist for a TF column at this `(model_version, tf_snapshot_id)`.

    A precondition failure (S4.0 exit ``3``) rather than a stage failure, because the
    stage has not begun: the alternative to registering frozen TF is Splink computing
    it from the corpus at hand, which breaks INV-SCORE silently and for every pair.
    The operator action is to re-materialize the snapshot, not to retry the run.
    """


def tf_columns(cfg: Config) -> tuple[str, ...]:
    """The `comparisons` keys carrying `tf: true`, in config order (S4.3.1).

    Config order, not sorted order: it is the order :func:`register_tf` registers in
    and the order a reader of `configs/*.yaml` expects to see reproduced. The flag is
    read off the attribute alone — S4.3.1 applies TF to the exact-match level of the
    named column, so no level token can turn it on or off.
    """
    return tuple(column for column, spec in cfg.comparisons.items() if spec.tf)


#: Process-wide, and monotonic so that two snapshots minted in one process sort in the
#: order they were taken. `tf_snapshot_id` is a `VARCHAR` ordered lexically wherever it
#: is compared, and ULIDs of one width sort as they were minted.
_TF_SNAPSHOT_FACTORY: Final[IdFactory] = MonotonicUlidFactory()


def new_tf_snapshot_id(factory: IdFactory | None = None) -> str:
    """Mint one `tf_snapshot_id` (D10, D4).

    THE mint site. D4 permits two callers — `er train`, and `er correct` via
    `er match --mode full --new-tf-snapshot` — and a guard test asserts there is no
    third: every extra mint is a TF boundary that `match_scores` rows on either side
    of it record having crossed only if the run that wrote them knew about it.

    Args:
        factory: an :class:`~er.entities.ids.IdFactory` for a caller that needs a
            reproducible sequence (tests). The shared monotonic ULID factory otherwise.
    """
    return (factory if factory is not None else _TF_SNAPSHOT_FACTORY).new()


def _reject_unknown_columns(columns: tuple[str, ...]) -> None:
    """Refuse a TF column `int_std_records` does not have (S6.1 V6, re-checked).

    The loader rejects it too, but a caller may hand this module a `Config` that never
    went through `load_config`, and the column name is interpolated into an identifier
    position below — a `comparisons` key is a config-authored string, and this is the
    only place in this module where one reaches SQL as anything but a parameter.
    """
    known = frozenset(STD_RECORD_COLUMNS)
    for column in columns:
        if column not in known:
            raise ConfigError(
                f"columns.unknown: /comparisons/{column}: {column!r} is not a column of "
                f"int_std_records (S5); the relation has {list(STD_RECORD_COLUMNS)}"
            )


def _frequency_branch(column: str) -> str:
    """One column's frozen rows, as a `SELECT` shaped like `tf_lookup`.

    The aggregate is wrapped in a subquery so the three key values stay bound
    parameters: constants beside a `GROUP BY` are a dialect question, and this way
    there is not one.

    The denominator is `count(<col>)`, which counts non-NULL values only — the same
    relative frequency Splink's own TF computation produces, so registering these rows
    scores identically to computing them over the same corpus.
    """
    return f"""
    SELECT ?, ?, ?, freq.value, freq.tf_value FROM (
        SELECT CAST({column} AS VARCHAR) AS value,
               CAST(count(*) AS DOUBLE)
                   / (SELECT count({column}) FROM {_STD_RECORDS}) AS tf_value
          FROM {_STD_RECORDS}
         WHERE {column} IS NOT NULL
         GROUP BY {column}
    ) AS freq
    """


_DELETE_KEY_SQL: Final = f"DELETE FROM {_TF_LOOKUP} WHERE model_version = ? AND tf_snapshot_id = ?"

_COUNT_KEY_SQL: Final = (
    f"SELECT count(*) FROM {_TF_LOOKUP} WHERE model_version = ? AND tf_snapshot_id = ?"
)

_SELECT_COLUMN_SQL: Final = f"""
SELECT value, tf_value FROM {_TF_LOOKUP}
 WHERE model_version = ? AND tf_snapshot_id = ? AND column_name = ?
 ORDER BY value
"""


def materialize_tf_lookup(
    connection: duckdb.DuckDBPyConnection,
    cfg: Config,
    model_version: str,
    tf_snapshot_id: str,
) -> int:
    """Freeze the corpus TF of every TF column under one key, and return the row count.

    One row per `(column_name, value)` over the non-NULL values of that column in
    `int_std_records`, with `tf_value` the value's relative frequency among them.
    Columns without `tf: true` contribute nothing.

    The key is deleted before it is written. DuckLake supports no `UNIQUE` constraint
    (S5.0), so `tf_lookup`'s logical key is enforced by the writer alone, and a second
    materialization under one key must replace rather than append — otherwise every
    value would carry two frequencies and the `register_term_frequency_lookup` join
    would double every corpus.

    Args:
        connection: a connection with the lake attached, per S4.0b.
        cfg: the validated S6 document; only `comparisons` is read.
        model_version: the zero-padded registry version these rows belong to.
        tf_snapshot_id: the id :func:`new_tf_snapshot_id` minted for them.

    Returns:
        How many rows `tf_lookup` holds for the key afterwards. Zero when the corpus
        has no non-NULL value in any TF column, which :func:`register_tf` will refuse.

    Raises:
        er.errors.ConfigError: a TF column is not a column of `int_std_records`.
    """
    columns = tf_columns(cfg)
    _reject_unknown_columns(columns)

    connection.execute(_DELETE_KEY_SQL, [model_version, tf_snapshot_id])
    if columns:
        # One statement over all columns rather than one per column: the write is a
        # DuckLake snapshot, and a snapshot per TF column would put two-thirds of this
        # stage's snapshot range there for no gain.
        branches = " UNION ALL ".join(_frequency_branch(column) for column in columns)
        parameters: list[str] = []
        for column in columns:
            parameters += [model_version, tf_snapshot_id, column]
        connection.execute(
            f"INSERT INTO {_TF_LOOKUP} "
            f"(model_version, tf_snapshot_id, column_name, value, tf_value) {branches}",
            parameters,
        )

    row = connection.execute(_COUNT_KEY_SQL, [model_version, tf_snapshot_id]).fetchone()
    assert row is not None, "count(*) returns a row"
    return int(row[0])


class TfTableManagement(Protocol):
    """The one Splink table-management call D4 permits a scoring run to make."""

    def register_term_frequency_lookup(
        self, input_data: Any, col_name: str, overwrite: bool = False
    ) -> Any:
        """Register a precomputed TF table for `col_name`."""


class TfLinker(Protocol):
    """A Splink `Linker`, narrowed to what :func:`register_tf` touches.

    A Protocol rather than the concrete `Linker`: the confinement claim of AC4 is
    about which calls are made, and a spy that records them is the only way to assert
    a *negative* — that Splink's own TF computation was not invoked.
    """

    @property
    def table_management(self) -> TfTableManagement:
        """The linker's table-management surface."""


def register_tf(
    linker: TfLinker,
    connection: duckdb.DuckDBPyConnection,
    cfg: Config,
    model_version: str,
    tf_snapshot_id: str,
) -> tuple[str, ...]:
    """Register the frozen TF rows of one key with `linker`, one call per TF column.

    This is the second half of D4 and the only sanctioned way a scoring run obtains
    term frequencies. Every TF column is registered — registering a subset is worse
    than registering none, because Splink would compute the rest from the corpus at
    hand and the resulting `match_probability` would be neither the frozen value nor a
    reproducible one.

    The rows are read fully qualified and materialized here rather than handed over as
    a lazy relation: Splink registers what it is given as a view, and a view over
    `lake.main.tf_lookup` would make every predict re-read the lake through a handle
    this function does not control.

    Args:
        linker: the Splink linker about to score.
        connection: a connection with the lake attached.
        cfg: the validated S6 document, which is what decides how many columns MUST be
            registered — the frozen rows cannot say which column is missing from them.
        model_version: the registry version to register.
        tf_snapshot_id: the TF snapshot to register.

    Returns:
        The columns registered, in :func:`tf_columns` order.

    Raises:
        MissingTfLookupError: a TF column has no rows for the key (S4.0 exit ``3``).
    """
    columns = tf_columns(cfg)
    for column in columns:
        rows = connection.execute(
            _SELECT_COLUMN_SQL, [model_version, tf_snapshot_id, column]
        ).fetchall()
        if not rows:
            raise MissingTfLookupError(
                f"{TF_LOOKUP_RELATION} holds no rows for column {column!r} at "
                f"model_version={model_version!r}, tf_snapshot_id={tf_snapshot_id!r}; "
                f"scoring without them would compute term frequency from the corpus at "
                f"hand and break INV-SCORE (S4.3.3, D4). Re-materialize the snapshot "
                f"named by {tf_tables_path(model_version, tf_snapshot_id)!r}"
            )
        # `<col>` and `tf_<col>` are the column names Splink joins the table on; the
        # value is text because `tf_lookup.value` is (S5).
        frame = [
            {column: str(value), f"{TF_COLUMN_PREFIX}{column}": float(tf_value)}
            for value, tf_value in rows
        ]
        # `overwrite=True` because a caller may register one linker twice — the S4.3.4
        # two-pass path builds a linker per pass, but a full re-score reuses one.
        linker.table_management.register_term_frequency_lookup(
            input_data=frame, col_name=column, overwrite=True
        )
    return columns


#: The `model_registry.tf_tables_path` format, produced and parsed nowhere else. The
#: relation is spelled from the same constants every query above uses, so a rename
#: cannot leave the locator pointing at the old name.
_TF_TABLES_PATH_TEMPLATE: Final = (
    f"{_TF_LOOKUP}?model_version={{model_version}}&tf_snapshot_id={{tf_snapshot_id}}"
)

#: Anything that would make the produced string un-parseable. Rejected on the way out
#: as well as on the way in, so :func:`parse_tf_tables_path` inverts
#: :func:`tf_tables_path` for every string the latter can return.
_PATH_RESERVED: Final = ("?", "&", "=")

_TF_TABLES_PATH_RE: Final = re.compile(
    re.escape(_TF_LOOKUP)
    + r"\?model_version=(?P<model_version>[^?&=]+)&tf_snapshot_id=(?P<tf_snapshot_id>[^?&=]+)$"
)


def tf_tables_path(model_version: str, tf_snapshot_id: str) -> str:
    """The `model_registry.tf_tables_path` locator for one TF key (S4.3.2, S5).

    Raises:
        ValueError: either component is empty or holds a character that separates the
            locator's own parts, which would make the string un-parseable.
    """
    for name, value in (("model_version", model_version), ("tf_snapshot_id", tf_snapshot_id)):
        if not value:
            raise ValueError(f"{name} must not be empty")
        for reserved in _PATH_RESERVED:
            if reserved in value:
                raise ValueError(f"{name} must not contain {reserved!r}: {value!r}")
    return _TF_TABLES_PATH_TEMPLATE.format(
        model_version=model_version, tf_snapshot_id=tf_snapshot_id
    )


def parse_tf_tables_path(path: str) -> tuple[str, str]:
    """Inverse of :func:`tf_tables_path`: `(model_version, tf_snapshot_id)`.

    Raises:
        ValueError: `path` is not a locator this module produced — a different
            relation, a missing or reordered parameter, or an empty component.
    """
    matched = _TF_TABLES_PATH_RE.fullmatch(path)
    if matched is None:
        raise ValueError(
            f"{path!r} is not a tf_tables_path; the format is "
            f"{_TF_TABLES_PATH_TEMPLATE.format(model_version='<mv>', tf_snapshot_id='<ts>')!r}"
        )
    return matched.group("model_version"), matched.group("tf_snapshot_id")
