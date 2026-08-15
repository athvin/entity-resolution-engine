"""S5.1's classification and its version-bump arm, both without a lake (S5, S5.1).

Two halves of S5.1 are decidable from data alone, and this is where they are decided:

* **What kind of change is this?** :func:`~er.lake.ddl.plan_evolution` takes a
  relation's live shape and its declared spec and returns a plan. The drop / rename /
  type-change / removed-domain-member quartet S5.1 calls breaking, and the added
  column it calls additive, are asserted here over hand-built registry pairs — because
  the alternative is discovering the classification only when a real lake happens to
  be in one of the five states, which is four states of coverage short.
* **What is the run that follows recorded as?**
  :func:`~er.versions.rebuild_reason_for` answers S5.1's version-bump rule, and its
  matrix is the one an operator meets after an edit: which of the five values, and
  why that one rather than the neighbouring one.

The pairs are hand-built rather than taken from :data:`~er.lake.model.REGISTRY` for
the reason S5.1's own arms are separate: a pair drawn from the live registry can only
ever be "unchanged", so a table built out of it would assert nothing about the four
breaking shapes. The one test that does read the registry is the statement-text one,
because the statement is only interesting if it is the statement `er init` will
actually issue.
"""

from __future__ import annotations

from dataclasses import replace
from types import MappingProxyType

import pytest

from er.lake.ddl import (
    ERR_SCHEMA_BREAKING,
    ChangeKind,
    SchemaBreakingError,
    add_column_statement,
    plan_evolution,
)
from er.lake.model import (
    BIGINT,
    DOUBLE,
    REBUILD_REASONS,
    REGISTRY,
    SCHEMA_QUALIFIER,
    VARCHAR,
    Column,
    LogicalKey,
    Owner,
    TableSpec,
)
from er.versions import (
    MODE_CORRECTION_PASS,
    MODE_FULL,
    MODE_INCREMENTAL,
    REBUILD_CORRECTION_PASS,
    REBUILD_OPERATOR,
    REBUILD_STD_VERSION_BUMP,
    REBUILD_SURVIVORSHIP_VERSION_BUMP,
    RunFingerprint,
    rebuild_reason_for,
)

#: The domain the live side of every pair below is written under. Three members, so
#: removing one still leaves a domain rather than emptying it — an empty domain would
#: be breaking for a second reason and the case would stop being about one difference.
LIVE_STATUSES = frozenset({"active", "merged", "retired"})

#: The same domain with one member removed. S5.1 calls this breaking because every
#: stored row carrying `retired` is orphaned by it.
NARROWED_STATUSES = frozenset({"active", "merged"})

#: The live relation every pair starts from. Deliberately not a relation of S5: these
#: are *registry pairs*, and using a real name would let a reader assume the verdict
#: came from S5 rather than from the two column lists in front of the function.
LIVE_COLUMNS: tuple[Column, ...] = (
    Column("entity_id", VARCHAR, nullable=False),
    Column("status", VARCHAR, nullable=False),
    Column("score", DOUBLE),
)

# The nullable column of `raw_records` an older lake would lack, and one of its
# `NOT NULL` columns. Both are added by the same statement shape, which is the whole
# content of S5.1's rule: DuckLake permits neither `NOT NULL` nor `DEFAULT` on an
# added column, so the registry's `NOT NULL` declarations apply at CREATE time only.
NULLABLE_COLUMN = "deleted_at"
NOT_NULL_COLUMN = "ingest_batch_id"

#: The fingerprint every `rebuild_reason_for` case starts from.
BASELINE = RunFingerprint(
    config_hash="a" * 64,
    model_version=None,
    std_version="1",
    survivorship_version="1",
)


def probe_spec(
    columns: tuple[Column, ...], *, statuses: frozenset[str] = LIVE_STATUSES
) -> TableSpec:
    """One side of a registry pair: the columns given, under the domain given."""
    return TableSpec(
        name="probe",
        owner=Owner.DDL,
        columns=columns,
        keys=(LogicalKey(("entity_id",)),),
        enums=MappingProxyType({"status": statuses}),
    )


LIVE = probe_spec(LIVE_COLUMNS)

#: The classification table: ``(label, live registry, declared registry, verdict)``.
#: Every row differs from :data:`LIVE` in exactly one way, so a failure names the shape
#: that was misclassified rather than a combination of two.
CASES: tuple[tuple[str, TableSpec, TableSpec, ChangeKind], ...] = (
    ("unchanged", LIVE, LIVE, ChangeKind.NOOP),
    (
        "added column",
        LIVE,
        probe_spec((*LIVE_COLUMNS, Column("note", VARCHAR))),
        ChangeKind.ADD_COLUMN,
    ),
    ("dropped column", LIVE, probe_spec(LIVE_COLUMNS[:2]), ChangeKind.BREAKING),
    (
        "renamed column",
        LIVE,
        probe_spec((*LIVE_COLUMNS[:2], Column("rating", DOUBLE))),
        ChangeKind.BREAKING,
    ),
    (
        "narrowed type",
        LIVE,
        probe_spec((*LIVE_COLUMNS[:2], Column("score", BIGINT))),
        ChangeKind.BREAKING,
    ),
    (
        "removed domain member",
        LIVE,
        probe_spec(LIVE_COLUMNS, statuses=NARROWED_STATUSES),
        ChangeKind.BREAKING,
    ),
)


def test_plan_evolution_classifies_additive_and_breaking() -> None:
    """AC8: the five shapes S5.1 names, plus the no-op, over hand-built pairs."""
    for label, live, declared, expected in CASES:
        plan = plan_evolution(live, declared)
        assert plan.kind is expected, f"{label}: {plan.differences}"
        assert plan.relation == "probe"

    # A rename is a drop AND an add, and the verdict has to be the drop's. Adding
    # `rating` while leaving `score` behind is the silent destructive migration S5.1
    # forbids, so `kind` may not be decided by whichever difference came first.
    renamed = plan_evolution(LIVE, probe_spec((*LIVE_COLUMNS[:2], Column("rating", DOUBLE))))
    assert [difference.column for difference in renamed.additive] == ["rating"]
    assert [difference.column for difference in renamed.breaking] == ["score"]
    assert renamed.kind is ChangeKind.BREAKING

    # The message is S5.1's, character for character, and it names the column the
    # operator has to go and migrate.
    narrowed = plan_evolution(LIVE, probe_spec((*LIVE_COLUMNS[:2], Column("score", BIGINT))))
    assert narrowed.breaking[0].message == f"{ERR_SCHEMA_BREAKING}: probe.score: DOUBLE -> BIGINT"

    # The domain arm renders both vocabularies where the types would go, so the
    # removed member is visible in the one message shape S5.1 gives every breaking
    # difference.
    domain = plan_evolution(LIVE, probe_spec(LIVE_COLUMNS, statuses=NARROWED_STATUSES))
    assert domain.breaking[0].column == "status"
    assert "retired" in domain.breaking[0].message
    assert domain.breaking[0].message.startswith(f"{ERR_SCHEMA_BREAKING}: probe.status: ")

    # A breaking plan has no statements at all. An empty tuple would let a caller
    # execute nothing and call the relation reconciled.
    with pytest.raises(SchemaBreakingError):
        _ = domain.statements


def test_plan_evolution_over_live_columns_cannot_see_a_domain() -> None:
    """The mapping arm compares columns only, which is all a live catalog can answer.

    DuckLake has no `ENUM` (S5.0): a domain is a dbt `accepted_values` test, so the
    catalog cannot be asked what vocabulary a relation was written under. That is why
    the removed-member arm above is reachable from a registry pair and never from
    :func:`~er.lake.ddl.preflight_schema` — and stating it here keeps it a documented
    limit rather than a silently missing check.
    """
    live_columns = {column.name: column.type for column in LIVE_COLUMNS}

    assert plan_evolution(live_columns, LIVE).kind is ChangeKind.NOOP
    assert (
        plan_evolution(live_columns, probe_spec(LIVE_COLUMNS, statuses=NARROWED_STATUSES)).kind
        is ChangeKind.NOOP
    )
    # The column half of the same mapping arm still classifies, so the limit above is
    # about domains and not about the arm being inert.
    assert (
        plan_evolution(live_columns, probe_spec((*LIVE_COLUMNS[:2], Column("score", BIGINT)))).kind
        is ChangeKind.BREAKING
    )


def test_added_column_ddl_has_no_not_null_and_no_default() -> None:
    """AC1: the emitted `ALTER` carries neither, whatever S5 declares on the column."""
    spec = REGISTRY["raw_records"]
    declared = {column.name: column for column in spec.columns}
    assert declared[NULLABLE_COLUMN].nullable
    # The vacuity guard: this column IS `NOT NULL` in S5, so a statement that carried
    # the declaration through would carry it here.
    assert not declared[NOT_NULL_COLUMN].nullable

    live = {
        column.name: column.type
        for column in spec.columns
        if column.name not in (NULLABLE_COLUMN, NOT_NULL_COLUMN)
    }
    plan = plan_evolution(live, spec)

    assert plan.kind is ChangeKind.ADD_COLUMN
    assert [difference.column for difference in plan.additive] == [
        NULLABLE_COLUMN,
        NOT_NULL_COLUMN,
    ], "the statements are emitted in S5 declaration order"

    statements = plan.statements
    assert statements == (
        f"ALTER TABLE {SCHEMA_QUALIFIER}.raw_records ADD COLUMN {NULLABLE_COLUMN} TIMESTAMP;",
        f"ALTER TABLE {SCHEMA_QUALIFIER}.raw_records ADD COLUMN {NOT_NULL_COLUMN} VARCHAR;",
    )
    for statement in statements:
        upper = statement.upper()
        assert "NOT NULL" not in upper, statement
        assert "DEFAULT" not in upper, statement

    # And the emitter refuses a difference that is not additive, so the no-NOT-NULL
    # guarantee cannot be reached with a type change smuggled through it.
    breaking = plan_evolution(
        {**{column.name: column.type for column in spec.columns}, "ingested_at": VARCHAR}, spec
    ).breaking
    with pytest.raises(ValueError, match=ERR_SCHEMA_BREAKING):
        add_column_statement(breaking[0])


def test_rebuild_reason_for_matrix() -> None:
    """AC5/AC6: the five values of `runs.rebuild_reason`, and why each one wins."""
    std_bump = replace(BASELINE, std_version="2")
    survivorship_bump = replace(BASELINE, survivorship_version="2")
    both = replace(BASELINE, std_version="2", survivorship_version="2")

    assert rebuild_reason_for(BASELINE, BASELINE) is None
    assert rebuild_reason_for(BASELINE, std_bump) == REBUILD_STD_VERSION_BUMP
    assert rebuild_reason_for(BASELINE, survivorship_bump) == REBUILD_SURVIVORSHIP_VERSION_BUMP
    # Standardization is upstream of survivorship, so a `std_version` bump already
    # implies re-assembly and naming the downstream half would understate the run.
    assert rebuild_reason_for(BASELINE, both) == REBUILD_STD_VERSION_BUMP

    # A first run builds; it does not rebuild. The bump is real and still unnamed,
    # because there is no prior corpus for it to have invalidated.
    assert rebuild_reason_for(None, std_bump) is None

    # `config_hash` drift is what the S4.0 guard catches, and it is NOT a corpus
    # rebuild: S5.1 ties the reason to the two derived-corpus versions and to nothing
    # else, so an edit to a threshold must not exempt the run from T-INC-2.
    assert rebuild_reason_for(BASELINE, replace(BASELINE, config_hash="b" * 64)) is None

    # `er correct` records its reason unconditionally (S4.0) and outranks a bump: the
    # pass rebuilds by definition, whatever the fingerprints say.
    assert rebuild_reason_for(BASELINE, BASELINE, mode=MODE_CORRECTION_PASS) == (
        REBUILD_CORRECTION_PASS
    )
    assert rebuild_reason_for(BASELINE, std_bump, mode=MODE_CORRECTION_PASS) == (
        REBUILD_CORRECTION_PASS
    )
    assert rebuild_reason_for(BASELINE, BASELINE, operator=True) == REBUILD_OPERATOR

    # The two `--mode` values are not reasons. A full run is not a planned rebuild
    # unless something was invalidated, or every `--mode full` would leave T-INC-2
    # with nothing to account for.
    for mode in (MODE_INCREMENTAL, MODE_FULL):
        assert rebuild_reason_for(BASELINE, BASELINE, mode=mode) is None

    # Every value this function can return is a member of S5's `accepted_values`
    # domain for the column it is written to, so a returned string cannot be one dbt
    # rejects at `dbt build` time.
    assert {
        REBUILD_STD_VERSION_BUMP,
        REBUILD_SURVIVORSHIP_VERSION_BUMP,
        REBUILD_CORRECTION_PASS,
        REBUILD_OPERATOR,
    } <= REBUILD_REASONS
