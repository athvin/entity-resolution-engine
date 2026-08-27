"""The S5 relation registry: every table, its owner, its columns and its keys.

This module is the single Python encoding of S5 and S5.0. `ddl.py` (ER-019) emits
`CREATE TABLE IF NOT EXISTS` from it, the dbt `sources.yml` / contract generator
reads the same specs, and every later stage projects columns from here instead of
restating a column list that would then drift from the spec.

Four facts are structural, not decorative:

* **Two owners, disjoint by construction** (S5.0, D14). A relation has exactly one
  :class:`TableSpec`, so :data:`DDL_OWNED` and :data:`DBT_OWNED` are partitions of
  :data:`REGISTRY` rather than two hand-maintained lists that can overlap.
* **Keys are logical** (B2). DuckLake enforces `NOT NULL` and nothing else — no
  `PRIMARY KEY`, `UNIQUE`, `FOREIGN KEY`, `CHECK`, index, sequence, `DEFAULT` or
  `ENUM` — so a key here is a :class:`LogicalKey` that dbt tests and `MERGE INTO`
  enforce, and :func:`create_table_sql` can emit no other constraint.
* **Every `in {…}` domain of the DDL is a plain `VARCHAR`** whose vocabulary lives
  on the spec's :attr:`TableSpec.enums`, for an `accepted_values` test to consume.
* **Column lists are never restated.** `golden_records`' survivable columns come
  from :data:`~er.lake.columns.GOLDEN_SURVIVABLE_COLUMNS`, and both the staging
  models and `int_std_records` share one payload tuple, because S5 declares them
  identical and two copies are how they stop being identical.

The module is pure: it opens no connection, executes nothing, and imports neither
`duckdb` nor `psycopg`. That is what lets it land before anything persists, and
what lets `tests/unit/test_ddl_registry.py` compare it against `DesignDoc.md`
without a lake. When the registry and S5 disagree, the registry is wrong.
"""

from __future__ import annotations

from collections.abc import Mapping
from dataclasses import dataclass, field
from enum import Enum
from types import MappingProxyType
from typing import Final

from er.lake.columns import GOLDEN_LINEAGE_ATTRIBUTES, GOLDEN_SURVIVABLE_COLUMNS

__all__ = [
    "ASSERTION_KINDS",
    "COLUMN_TYPES",
    "DBT_OWNED",
    "DDL_OWNED",
    "DISPOSITIONS",
    "ENTITY_STATUSES",
    "EVENT_TYPES",
    "GOLDEN_LINEAGE_ATTRIBUTES",
    "MODEL_STATUSES",
    "PAIR_RELATIONS",
    "PROMOTED_COUNTERS",
    "REBUILD_REASONS",
    "REGISTRY",
    "REVIEW_REASONS",
    "REVIEW_STATUSES",
    "REVIEW_SUBJECT_TYPES",
    "RUN_MODES",
    "RUN_STAGES",
    "RUN_STATUSES",
    "SCHEMA_QUALIFIER",
    "STG_SOURCES",
    "Column",
    "LogicalKey",
    "Owner",
    "TableSpec",
    "create_table_sql",
    "logical_key",
]

#: Every relation lives in the attached catalog under alias `lake`, schema `main`
#: (S5). Spelled here rather than imported from ``er.lake.ducklake``, which owns
#: the ATTACH and therefore imports duckdb; this module must stay importable
#: without it. ``tests/unit/test_ddl_registry.py`` asserts the two agree.
SCHEMA_QUALIFIER: Final = "lake.main"

# The eight types S5 permits. Fixed-size `ARRAY` is deliberately absent: DuckLake
# does not support it (S5.0), while `JSON` and `LIST(VARCHAR)` are retained.
VARCHAR: Final = "VARCHAR"
BIGINT: Final = "BIGINT"
DOUBLE: Final = "DOUBLE"
BOOLEAN: Final = "BOOLEAN"
DATE: Final = "DATE"
TIMESTAMP: Final = "TIMESTAMP"
JSON: Final = "JSON"
LIST_VARCHAR: Final = "LIST(VARCHAR)"

COLUMN_TYPES: Final[frozenset[str]] = frozenset(
    {VARCHAR, BIGINT, DOUBLE, BOOLEAN, DATE, TIMESTAMP, JSON, LIST_VARCHAR}
)


class Owner(Enum):
    """The two — and only two — owners of a relation (S5.0, D14).

    `ddl.py` never issues DDL against a dbt-owned relation, and dbt never
    materializes a `ddl.py`-owned one; S5 stays the review-time authority for both.
    """

    DDL = "ddl.py"
    DBT = "dbt"


@dataclass(frozen=True, slots=True)
class Column:
    """One column of a relation, in its S5 declaration order.

    ``nullable`` is the whole constraint model: `NOT NULL` is the only constraint
    DuckLake enforces, so it is the only one representable here.
    """

    name: str
    type: str
    nullable: bool = True

    def __post_init__(self) -> None:
        if self.type not in COLUMN_TYPES:
            raise ValueError(f"{self.name}: {self.type!r} is not one of the S5 types")

    @property
    def sql(self) -> str:
        """The column's DDL fragment, `NOT NULL` included when it applies."""
        return f"{self.name} {self.type}{'' if self.nullable else ' NOT NULL'}"


@dataclass(frozen=True, slots=True)
class LogicalKey:
    """A key S5.0 declares, enforced by a dbt test and by `MERGE INTO` — never by the engine.

    ``where`` carries the filter of a *partial* key verbatim as S5.0 writes it
    (``"active"``, ``"status='open'"``). ``columns`` is empty only for
    `model_registry`'s "at most one row with ``status='active'``": uniqueness over
    the empty tuple under a filter is exactly the at-most-one-row claim, and it is
    the singular test S5.0 pairs with that row.
    """

    columns: tuple[str, ...]
    where: str | None = None


_NO_ENUMS: Final[Mapping[str, frozenset[str]]] = MappingProxyType({})


@dataclass(frozen=True, slots=True)
class TableSpec:
    """One relation: its owner, its ordered columns, its logical keys, its domains."""

    name: str
    owner: Owner
    columns: tuple[Column, ...]
    keys: tuple[LogicalKey, ...]
    enums: Mapping[str, frozenset[str]] = field(default=_NO_ENUMS)

    def __post_init__(self) -> None:
        names = self.column_names
        if len(set(names)) != len(names):
            raise ValueError(f"{self.name}: duplicate column")
        for key in self.keys:
            for column in key.columns:
                if column not in names:
                    raise ValueError(f"{self.name}: key column {column!r} is not a column")
        for column in self.enums:
            if column not in names:
                raise ValueError(f"{self.name}: enum column {column!r} is not a column")

    @property
    def column_names(self) -> tuple[str, ...]:
        return tuple(column.name for column in self.columns)


def _nn(name: str, type_: str) -> Column:
    """A `NOT NULL` column — the only constraint any generated statement may carry."""
    return Column(name, type_, nullable=False)


# --------------------------------------------------------------------------- #
# shared column groups
# --------------------------------------------------------------------------- #

# The eleven promoted counters of S5.2: the CLOSED set that is a typed column of
# `run_stages`. Each is a nullable BIGINT — a stage writes NULL for the counters
# that do not apply to it. A stage may invent a name in `run_stages.counters`
# (JSON); it may never invent a column, because promoting one is a schema change
# under S5.1. They are contiguous and in this order in the S5 DDL, so `run_stages`
# builds its counter columns straight from this tuple.
PROMOTED_COUNTERS: Final[tuple[str, ...]] = (
    "rows_in",
    "rows_out",
    "candidate_pairs",
    "pairs_above_auto_merge",
    "entities_created",
    "entities_merged",
    "entities_split",
    "entities_retired",
    "edges_cut",
    "review_queue_added",
    "duration_ms",
)

#: The four relations carrying the `rec_a_key < rec_b_key` invariant (S5.0, D9).
#: Canonicalisation happens in one helper at write time (``er.entities.ids``), so
#: readers of these four never perform a two-sided join.
PAIR_RELATIONS: Final[frozenset[str]] = frozenset(
    {"match_scores", "assertions", "review_queue", "cut_edges"}
)

#: The three sources S5 names for the identically-shaped `stg_<source>` models.
STG_SOURCES: Final[tuple[str, ...]] = ("crm", "billing", "webforms")

# The standardized payload S5 declares once for `stg_<source>` and reuses verbatim
# for `int_std_records`, which prefixes it with `record_key`. Declaring it twice is
# how the staging contract and the intermediate contract would drift apart.
_STD_PAYLOAD: Final[tuple[Column, ...]] = (
    _nn("source_system", VARCHAR),
    _nn("source_record_id", VARCHAR),
    _nn("content_hash", VARCHAR),
    _nn("std_version", VARCHAR),
    Column("given_name", VARCHAR),
    Column("family_name", VARCHAR),
    _nn("name_variants", LIST_VARCHAR),
    Column("email", VARCHAR),
    Column("email_valid", BOOLEAN),
    Column("phone_e164", VARCHAR),
    Column("phone_valid", BOOLEAN),
    Column("addr_number", VARCHAR),
    Column("addr_street", VARCHAR),
    Column("addr_unit", VARCHAR),
    Column("addr_city", VARCHAR),
    Column("addr_region", VARCHAR),
    Column("addr_postal", VARCHAR),
    Column("birth_date", DATE),
    Column("updated_at_source", TIMESTAMP),
    _nn("ingest_batch_id", VARCHAR),
    _nn("ingested_at", TIMESTAMP),
)

_STD_PAYLOAD_TYPES: Final[Mapping[str, str]] = MappingProxyType(
    {column.name: column.type for column in _STD_PAYLOAD}
)

# `golden_records`' survivable columns are the standardized columns that won
# survivorship, so they take both their names and their types from the corpus they
# are copied out of: the names from `columns.py` (S5.0's single definition), the
# types from the payload above. Neither list is restated here.
_GOLDEN_SURVIVABLE: Final[tuple[Column, ...]] = tuple(
    Column(name, _STD_PAYLOAD_TYPES[name]) for name in GOLDEN_SURVIVABLE_COLUMNS
)

# --------------------------------------------------------------------------- #
# enum vocabularies -- every `in {…}` domain the S5 DDL declares
# --------------------------------------------------------------------------- #

#: `member_removed` and `retired` are reachable because deletion is in v1 scope
#: (D8): a tombstoned record departs its entity, and an emptied entity retires.
#: `edge_cut` records an S4.4.2 partition-level `never` cut (D5).
EVENT_TYPES: Final[frozenset[str]] = frozenset(
    {"created", "member_added", "member_removed", "merged", "split", "retired", "edge_cut"}
)

ENTITY_STATUSES: Final[frozenset[str]] = frozenset({"active", "merged", "retired"})

ASSERTION_KINDS: Final[frozenset[str]] = frozenset({"always", "never"})

REVIEW_SUBJECT_TYPES: Final[frozenset[str]] = frozenset({"pair", "entity"})

#: `gray_band` is S4.3.5, `never_unsatisfiable` the S4.4.2 escalation, `coherence`
#: the phase-2 scorer's. They are independent reasons, which is why `reason` is in
#: `review_queue`'s open-row key (M20) rather than collapsed into one row per pair.
REVIEW_REASONS: Final[frozenset[str]] = frozenset({"gray_band", "never_unsatisfiable", "coherence"})

REVIEW_STATUSES: Final[frozenset[str]] = frozenset(
    {"open", "resolved_match", "resolved_no_match", "dismissed"}
)

MODEL_STATUSES: Final[frozenset[str]] = frozenset({"active", "superseded"})

#: `stage` covers every standalone stage invocation outside a `run-all` chain;
#: `run_stages` then holds exactly one row naming which stage it was (S5).
RUN_MODES: Final[frozenset[str]] = frozenset(
    {"incremental", "full", "train", "init", "maintain", "reset", "correction_pass", "stage"}
)

RUN_STAGES: Final[frozenset[str]] = frozenset(
    {
        "init",
        "ingest",
        "standardize",
        "train",
        "match",
        "reconcile",
        "assemble",
        "maintain",
        "reset",
    }
)

RUN_STATUSES: Final[frozenset[str]] = frozenset({"running", "succeeded", "failed"})

#: A non-NULL `rebuild_reason` puts the run outside T-INC-2's accounting (S5.1).
REBUILD_REASONS: Final[frozenset[str]] = frozenset(
    {"std_version_bump", "survivorship_version_bump", "correction_pass", "operator"}
)

#: `rebuild` entities are re-assembled by the marts; `retire` entities are deleted
#: from the three golden relations afterwards, because `delete+insert` cannot
#: remove a key absent from the incoming batch (S5.2, M10).
DISPOSITIONS: Final[frozenset[str]] = frozenset({"rebuild", "retire"})

#: `address` is one lineage attribute covering the six `addr_*` columns, which S4.6
#: requires to come from a single winning record.
#:
#: Imported rather than listed. The vocabulary is a CONSEQUENCE of S5's survivable
#: column set -- those eleven columns with the six `addr_*` ones collapsed -- so
#: `columns.py` derives it there and this module re-exports it. Two listings could
#: disagree about a column added to S5, and the enum below is what a `dbt build`
#: would then accept.

# --------------------------------------------------------------------------- #
# the relations, in S5 declaration order
# --------------------------------------------------------------------------- #

_DDL_SPECS: Final[tuple[TableSpec, ...]] = (
    TableSpec(
        name="raw_records",
        owner=Owner.DDL,
        columns=(
            _nn("source_system", VARCHAR),
            _nn("source_record_id", VARCHAR),
            _nn("payload", JSON),
            _nn("content_hash", VARCHAR),
            _nn("is_deleted", BOOLEAN),
            Column("deleted_at", TIMESTAMP),
            _nn("ingest_batch_id", VARCHAR),
            _nn("ingested_at", TIMESTAMP),
        ),
        # Append-only version history, so `content_hash` is in the key and the
        # relation carries no update column (D7).
        keys=(LogicalKey(("source_system", "source_record_id", "content_hash")),),
    ),
    TableSpec(
        name="match_scores",
        owner=Owner.DDL,
        columns=(
            _nn("rec_a_key", VARCHAR),
            _nn("rec_b_key", VARCHAR),
            _nn("match_probability", DOUBLE),
            _nn("model_version", VARCHAR),
            _nn("tf_snapshot_id", VARCHAR),
            _nn("rec_a_content_hash", VARCHAR),
            _nn("rec_b_content_hash", VARCHAR),
            _nn("evidence", JSON),
            _nn("is_active", BOOLEAN),
            Column("invalidated_at", TIMESTAMP),
            Column("invalidated_run_id", VARCHAR),
            _nn("run_id", VARCHAR),
            _nn("scored_at", TIMESTAMP),
        ),
        # Unfiltered on purpose: it is the S4.3.4 `MERGE INTO` key, and
        # invalidation updates `is_active` in place rather than adding a row.
        keys=(LogicalKey(("rec_a_key", "rec_b_key", "model_version", "tf_snapshot_id")),),
    ),
    TableSpec(
        name="entity_membership",
        owner=Owner.DDL,
        columns=(
            _nn("source_system", VARCHAR),
            _nn("source_record_id", VARCHAR),
            _nn("record_key", VARCHAR),
            _nn("entity_id", VARCHAR),
            _nn("assigned_at", TIMESTAMP),
            _nn("run_id", VARCHAR),
        ),
        # CURRENT STATE: one row per record, maintained by `MERGE INTO`; all
        # history lives in `entity_events` (D3, S4.5.3).
        keys=(LogicalKey(("source_system", "source_record_id")),),
    ),
    TableSpec(
        name="entities",
        owner=Owner.DDL,
        columns=(
            _nn("entity_id", VARCHAR),
            _nn("status", VARCHAR),
            Column("merged_into", VARCHAR),
            _nn("created_at", TIMESTAMP),
            _nn("updated_at", TIMESTAMP),
            _nn("created_run_id", VARCHAR),
            _nn("updated_run_id", VARCHAR),
        ),
        keys=(LogicalKey(("entity_id",)),),
        enums=MappingProxyType({"status": ENTITY_STATUSES}),
    ),
    TableSpec(
        name="entity_events",
        owner=Owner.DDL,
        columns=(
            _nn("event_id", VARCHAR),
            _nn("seq", BIGINT),
            _nn("run_id", VARCHAR),
            _nn("entity_id", VARCHAR),
            _nn("event_type", VARCHAR),
            _nn("details", JSON),
            _nn("details_hash", VARCHAR),
            _nn("occurred_at", TIMESTAMP),
        ),
        # The second key is the idempotency key of S4.5.4: a re-run producing
        # identical output writes zero events.
        keys=(
            LogicalKey(("event_id",)),
            LogicalKey(("run_id", "entity_id", "event_type", "details_hash")),
        ),
        enums=MappingProxyType({"event_type": EVENT_TYPES}),
    ),
    TableSpec(
        name="assertions",
        owner=Owner.DDL,
        columns=(
            _nn("assertion_id", VARCHAR),
            _nn("rec_a_key", VARCHAR),
            _nn("rec_b_key", VARCHAR),
            _nn("kind", VARCHAR),
            _nn("active", BOOLEAN),
            _nn("created_by", VARCHAR),
            _nn("created_at", TIMESTAMP),
            Column("retracted_by", VARCHAR),
            Column("retracted_at", TIMESTAMP),
            Column("note", VARCHAR),
        ),
        keys=(
            LogicalKey(("assertion_id",)),
            LogicalKey(("rec_a_key", "rec_b_key"), where="active"),
        ),
        enums=MappingProxyType({"kind": ASSERTION_KINDS}),
    ),
    TableSpec(
        name="review_queue",
        owner=Owner.DDL,
        columns=(
            _nn("review_id", VARCHAR),
            _nn("subject_type", VARCHAR),
            Column("rec_a_key", VARCHAR),
            Column("rec_b_key", VARCHAR),
            Column("entity_id", VARCHAR),
            _nn("reason", VARCHAR),
            Column("match_probability", DOUBLE),
            Column("waterfall", JSON),
            _nn("status", VARCHAR),
            _nn("first_seen_run_id", VARCHAR),
            _nn("last_seen_run_id", VARCHAR),
            Column("resolved_by", VARCHAR),
            Column("resolved_at", TIMESTAMP),
        ),
        # `reason` is in the open-row tuple because one pair can be open for two
        # independent reasons; collapsing them would drop a steward task (M20).
        keys=(
            LogicalKey(("review_id",)),
            LogicalKey(
                ("subject_type", "rec_a_key", "rec_b_key", "entity_id", "reason"),
                where="status='open'",
            ),
        ),
        enums=MappingProxyType(
            {
                "subject_type": REVIEW_SUBJECT_TYPES,
                "reason": REVIEW_REASONS,
                "status": REVIEW_STATUSES,
            }
        ),
    ),
    TableSpec(
        name="model_registry",
        owner=Owner.DDL,
        columns=(
            _nn("model_version", VARCHAR),
            _nn("status", VARCHAR),
            _nn("trained_at", TIMESTAMP),
            _nn("corpus_snapshot", BIGINT),
            _nn("params_path", VARCHAR),
            _nn("tf_tables_path", VARCHAR),
            _nn("tf_snapshot_id", VARCHAR),
            _nn("config_hash", VARCHAR),
            _nn("metrics", JSON),
            _nn("run_id", VARCHAR),
        ),
        # The second key is "at most one active row": no columns, one filter.
        keys=(
            LogicalKey(("model_version",)),
            LogicalKey((), where="status='active'"),
        ),
        enums=MappingProxyType({"status": MODEL_STATUSES}),
    ),
    TableSpec(
        name="tf_lookup",
        owner=Owner.DDL,
        columns=(
            _nn("model_version", VARCHAR),
            _nn("tf_snapshot_id", VARCHAR),
            _nn("column_name", VARCHAR),
            _nn("value", VARCHAR),
            _nn("tf_value", DOUBLE),
        ),
        keys=(LogicalKey(("model_version", "tf_snapshot_id", "column_name", "value")),),
    ),
    TableSpec(
        name="cut_edges",
        owner=Owner.DDL,
        columns=(
            _nn("cut_id", VARCHAR),
            _nn("rec_a_key", VARCHAR),
            _nn("rec_b_key", VARCHAR),
            _nn("match_probability", DOUBLE),
            Column("model_version", VARCHAR),
            Column("tf_snapshot_id", VARCHAR),
            _nn("assertion_id", VARCHAR),
            _nn("active", BOOLEAN),
            _nn("cut_run_id", VARCHAR),
            _nn("cut_at", TIMESTAMP),
            Column("released_run_id", VARCHAR),
            Column("released_at", TIMESTAMP),
        ),
        # Active cuts are excluded from the clustering edge set on every later run;
        # without that, every cut is re-merged next run and `never` becomes a no-op
        # with a one-run half-life (S4.4.2, D5).
        keys=(
            LogicalKey(("cut_id",)),
            LogicalKey(("rec_a_key", "rec_b_key"), where="active"),
        ),
    ),
    TableSpec(
        name="runs",
        owner=Owner.DDL,
        columns=(
            _nn("run_id", VARCHAR),
            _nn("tenant", VARCHAR),
            _nn("mode", VARCHAR),
            _nn("status", VARCHAR),
            _nn("started_at", TIMESTAMP),
            Column("ended_at", TIMESTAMP),
            _nn("config_hash", VARCHAR),
            Column("model_version", VARCHAR),
            Column("tf_snapshot_id", VARCHAR),
            _nn("std_version", VARCHAR),
            _nn("survivorship_version", VARCHAR),
            _nn("code_version", VARCHAR),
            Column("rebuild_reason", VARCHAR),
            Column("snapshot_start", BIGINT),
            Column("snapshot_end", BIGINT),
        ),
        keys=(LogicalKey(("run_id",)),),
        enums=MappingProxyType(
            {"mode": RUN_MODES, "status": RUN_STATUSES, "rebuild_reason": REBUILD_REASONS}
        ),
    ),
    TableSpec(
        name="run_stages",
        owner=Owner.DDL,
        columns=(
            _nn("run_id", VARCHAR),
            _nn("stage", VARCHAR),
            _nn("seq", BIGINT),
            _nn("status", VARCHAR),
            _nn("started_at", TIMESTAMP),
            Column("ended_at", TIMESTAMP),
            Column("snapshot_start", BIGINT),
            Column("snapshot_end", BIGINT),
            *(Column(counter, BIGINT) for counter in PROMOTED_COUNTERS),
            Column("counters", JSON),
            Column("error_class", VARCHAR),
            Column("error_detail", VARCHAR),
        ),
        # `(run_id, seq)` orders the stages of a run; `(run_id, stage)` is what
        # makes a stage's snapshot range addressable exactly once per run.
        keys=(LogicalKey(("run_id", "seq")), LogicalKey(("run_id", "stage"))),
        enums=MappingProxyType({"stage": RUN_STAGES, "status": RUN_STATUSES}),
    ),
    TableSpec(
        name="ingest_batches",
        owner=Owner.DDL,
        columns=(
            _nn("ingest_batch_id", VARCHAR),
            _nn("run_id", VARCHAR),
            _nn("source_system", VARCHAR),
            _nn("path", VARCHAR),
            _nn("new_count", BIGINT),
            _nn("changed_count", BIGINT),
            _nn("unchanged_count", BIGINT),
            _nn("tombstone_count", BIGINT),
            # The fifth count of S4.1.1, persisted rather than manifest-only.
            _nn("resurrected_count", BIGINT),
            _nn("full_refresh_keys", BOOLEAN),
            _nn("created_at", TIMESTAMP),
        ),
        keys=(LogicalKey(("ingest_batch_id",)),),
    ),
    TableSpec(
        name="er_touched_entities",
        owner=Owner.DDL,
        columns=(
            _nn("run_id", VARCHAR),
            _nn("entity_id", VARCHAR),
            _nn("disposition", VARCHAR),
            _nn("created_at", TIMESTAMP),
        ),
        # Joined by the marts on `run_id`, because a `--vars` payload of the
        # touched set would exceed the argv limit (S5.2, M10).
        keys=(LogicalKey(("run_id", "entity_id")),),
        enums=MappingProxyType({"disposition": DISPOSITIONS}),
    ),
)


def _stg_spec(source: str) -> TableSpec:
    """One `stg_<source>` model: S5 declares the three identical in shape."""
    return TableSpec(
        name=f"stg_{source}",
        owner=Owner.DBT,
        columns=_STD_PAYLOAD,
        keys=(LogicalKey(("source_system", "source_record_id", "content_hash")),),
    )


_DBT_SPECS: Final[tuple[TableSpec, ...]] = (
    *(_stg_spec(source) for source in STG_SOURCES),
    TableSpec(
        name="int_std_records",
        owner=Owner.DBT,
        # `record_key` leads: it is the logical key and Splink's
        # `unique_id_column_name` (S5.0, D6).
        columns=(_nn("record_key", VARCHAR), *_STD_PAYLOAD),
        keys=(LogicalKey(("record_key",)),),
    ),
    TableSpec(
        name="int_blocking_keys",
        owner=Owner.DBT,
        columns=(
            _nn("key_type", VARCHAR),
            _nn("key_value", VARCHAR),
            _nn("record_key", VARCHAR),
            _nn("source_system", VARCHAR),
            _nn("source_record_id", VARCHAR),
        ),
        keys=(LogicalKey(("key_type", "key_value", "record_key")),),
    ),
    TableSpec(
        name="golden_records",
        owner=Owner.DBT,
        columns=(
            _nn("entity_id", VARCHAR),
            *_GOLDEN_SURVIVABLE,
            _nn("survivorship_version", VARCHAR),
            _nn("assembled_at", TIMESTAMP),
        ),
        keys=(LogicalKey(("entity_id",)),),
    ),
    TableSpec(
        name="golden_lineage",
        owner=Owner.DBT,
        columns=(
            _nn("entity_id", VARCHAR),
            _nn("attribute", VARCHAR),
            _nn("record_key", VARCHAR),
            _nn("source_system", VARCHAR),
            _nn("source_record_id", VARCHAR),
            _nn("rule", VARCHAR),
            _nn("survivorship_version", VARCHAR),
            _nn("assembled_at", TIMESTAMP),
        ),
        keys=(LogicalKey(("entity_id", "attribute")),),
        enums=MappingProxyType({"attribute": frozenset(GOLDEN_LINEAGE_ATTRIBUTES)}),
    ),
    TableSpec(
        name="golden_display",
        owner=Owner.DBT,
        # Presentation casing only; the matching layer MUST never read it (S5).
        columns=(
            _nn("entity_id", VARCHAR),
            Column("display_name", VARCHAR),
            Column("display_email", VARCHAR),
            Column("display_phone", VARCHAR),
            Column("display_address", VARCHAR),
            _nn("assembled_at", TIMESTAMP),
        ),
        keys=(LogicalKey(("entity_id",)),),
    ),
)


def _build_registry(specs: tuple[TableSpec, ...]) -> Mapping[str, TableSpec]:
    """Index the specs by name, refusing a relation claimed twice.

    A dict comprehension would silently keep the last of two same-named specs,
    which is exactly the two-owners-for-one-relation defect M4 records (D14).
    """
    by_name: dict[str, TableSpec] = {}
    for spec in specs:
        if spec.name in by_name:
            raise ValueError(f"{spec.name} is declared twice; a relation has exactly one owner")
        by_name[spec.name] = spec
    return MappingProxyType(by_name)


#: Every relation of S5, in declaration order, keyed by name.
REGISTRY: Final[Mapping[str, TableSpec]] = _build_registry((*_DDL_SPECS, *_DBT_SPECS))

#: The two owner partitions. Derived from :data:`REGISTRY` rather than listed, so
#: a relation cannot appear under both owners (D14).
DDL_OWNED: Final[tuple[str, ...]] = tuple(
    name for name, spec in REGISTRY.items() if spec.owner is Owner.DDL
)
DBT_OWNED: Final[tuple[str, ...]] = tuple(
    name for name, spec in REGISTRY.items() if spec.owner is Owner.DBT
)


def create_table_sql(spec: TableSpec) -> str:
    """The `CREATE TABLE IF NOT EXISTS` statement for a `ddl.py`-owned relation.

    Pure text: nothing here connects or executes. `NOT NULL` is the only constraint
    it can emit, because :class:`Column` can represent no other (S5.0).

    Raises:
        ValueError: if ``spec`` is dbt-owned. dbt materializes those under an
            enforced contract, and `ddl.py` issuing DDL against one is the
            ownership violation D14 forbids.
    """
    if spec.owner is not Owner.DDL:
        raise ValueError(f"{spec.name} is dbt-owned; ddl.py must never issue DDL against it")
    body = ",\n".join(f"  {column.sql}" for column in spec.columns)
    return f"CREATE TABLE IF NOT EXISTS {SCHEMA_QUALIFIER}.{spec.name} (\n{body}\n);"


def logical_key(relation: str) -> tuple[LogicalKey, ...]:
    """The logical keys S5.0 declares for ``relation``, in the order it lists them.

    Raises:
        KeyError: if ``relation`` is not a relation of S5.
    """
    return REGISTRY[relation].keys
