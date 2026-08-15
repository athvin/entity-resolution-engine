"""Applying the S5 registry to a live namespace (DesignDoc.md S5, S5.0, S5.1).

`er init` and every stage preflight reach the lake through this module. It does
three things and refuses to do a fourth:

* **Create.** `CREATE TABLE IF NOT EXISTS` for the fourteen `ddl.py`-owned
  relations, in registry order. Running it against an initialised lake is a no-op
  that reports `exists` and executes nothing (S5.1).
* **Reconcile, additively.** The only change S5.1 permits against an existing
  relation is `ALTER TABLE … ADD COLUMN <name> <type>` with **no** `NOT NULL` and
  **no** `DEFAULT`, because DuckLake permits neither on an added column. The
  owning stage backfills the new column, or it stays NULL.
* **Abort on a breaking difference.** A dropped, renamed, narrowed or re-typed
  column — and a live column the registry no longer declares — raises
  :class:`SchemaBreakingError` with the literal S5.1 message and exit code ``3``,
  *before* any statement executes. There is no automatic destructive migration;
  the operator path is a migration script plus a full rebuild.

The fourth thing is issuing DDL against a dbt-owned relation, which S5.0 forbids:
dbt materializes those under an enforced contract, and two systems claiming one
relation is the M4 defect this split exists to close. Every emitter here goes
through :func:`assert_ddl_owned`, so the refusal is a guard rather than a habit.

The classification the last two bullets rest on is :func:`plan_evolution`, and it is
**pure**: two column lists in, an :class:`EvolutionPlan` out, no connection anywhere.
That is what lets the additive/breaking decision — the one that turns a run into an
exit ``3`` — be asserted on a bare runner over hand-built registry pairs, and it is
why :func:`preflight_schema`, the entry path `er init` and every stage run through,
is a loop over it rather than a second opinion about the same question.

Every statement is derived from :mod:`er.lake.model`. Nothing here restates a
column list: a second copy of the S5 columns is exactly how the registry and the
live catalog would stop agreeing, which is the drift the diff below is meant to
detect rather than to cause.
"""

from __future__ import annotations

from collections.abc import Mapping
from dataclasses import dataclass
from enum import StrEnum
from types import MappingProxyType
from typing import Final

import duckdb

from er.errors import ErrorClass
from er.lake.model import (
    DDL_OWNED,
    LIST_VARCHAR,
    REGISTRY,
    SCHEMA_QUALIFIER,
    Owner,
    TableSpec,
    create_table_sql,
)

__all__ = [
    "ERR_SCHEMA_BREAKING",
    "ChangeKind",
    "ColumnDiff",
    "EvolutionPlan",
    "OwnershipViolation",
    "RelationAction",
    "RelationResult",
    "SchemaBreakingError",
    "add_column_statement",
    "apply",
    "assert_ddl_owned",
    "create_statement",
    "diff",
    "evolve",
    "live_columns",
    "plan_evolution",
    "preflight_schema",
]

# S5 puts every relation under `lake.main`; `model.py` owns that spelling and this
# module splits it rather than repeating the two halves, so a namespace change has
# one edit site.
_DATABASE, _SCHEMA = SCHEMA_QUALIFIER.split(".", 1)

# What DuckDB's catalog views report for a type S5 spells differently. This is the
# engine's spelling of the same type, not a second type system: `duckdb_columns()`
# renders a list type in postfix form. Only `LIST(VARCHAR)` differs today, and it
# appears on dbt-owned relations only, so nothing here can emit it — the entry
# exists so that a live dbt-owned relation read through `live_columns` is reported
# in terms a reader can compare against S5.
_CATALOG_SPELLING: Final[Mapping[str, str]] = MappingProxyType({LIST_VARCHAR: "VARCHAR[]"})

# Rendered in the S5.1 message where the declared type would go, for the one
# breaking difference that has no declared type: a live column the registry does
# not declare. The message shape stays `<live_type> -> <declared_type>`.
_UNDECLARED: Final[str] = "(undeclared)"

# How a domain renders in the two type slots of the S5.1 message. S5.1 makes
# "removing a value from an `accepted_values` domain" breaking in the same breath as
# a re-typed column and gives all of them ONE message shape, so the vocabulary goes
# where the type does rather than earning a second sentence an operator has to learn.
_DOMAIN_RENDERING: Final[str] = "accepted_values({members})"

#: The literal token S5.1 names. Operators and tests match on it, so it is spelled
#: once.
ERR_SCHEMA_BREAKING: Final[str] = "ERR_SCHEMA_BREAKING"


class RelationAction(StrEnum):
    """What :func:`apply` did to one relation.

    A :class:`StrEnum` because these two words are `er init`'s stdout contract
    (S4.0: "One line per relation: `created` / `exists`") and its ``--json``
    payload; the member value IS the printed spelling.
    """

    CREATED = "created"
    EXISTS = "exists"


class ChangeKind(StrEnum):
    """S5.1's verdict on one relation's difference set.

    Three values and not two: ``noop`` is what an idempotent `er init` must find on
    an unchanged lake, and folding it into ``add_column`` with an empty statement
    list would make "issued zero DDL" indistinguishable from "issued none of the
    statements it planned".
    """

    NOOP = "noop"
    ADD_COLUMN = "add_column"
    BREAKING = "breaking"


class OwnershipViolation(ValueError):
    """DDL was requested for a relation `ddl.py` does not own (S5.0).

    A ``ValueError`` because it can only be a caller bug — the registry knows
    every relation's owner statically, so no lake state and no operator action can
    produce or repair one. :func:`~er.lake.model.create_table_sql` refuses the same
    condition with the same base class; this is its named form.
    """


class SchemaBreakingError(Exception):
    """A non-additive difference between the registry and the live catalog (S5.1).

    Carries the S5.1 message verbatim —
    ``ERR_SCHEMA_BREAKING: <relation>.<column>: <live_type> -> <declared_type>`` —
    and exit code ``3``, since S4.7 classes a breaking schema change as a
    ``precondition`` failure: no snapshot is committed and the operator fixes the
    schema before re-running.

    Not a subclass of :class:`~er.errors.PreconditionFailure`: the exit code and
    the error class come from the same S4.7 row either way, and this module is
    reachable from `er init`'s preflight before any `run_stages` row exists to
    record a class against.
    """

    #: The S4.7 row this failure is recorded under, and the S4.0 status it exits
    #: with. Spelled as attributes rather than inherited so that
    #: :func:`~er.errors.exit_code_for` honours the ``code`` without this module
    #: importing the exception hierarchy.
    error_class: Final = ErrorClass.PRECONDITION
    code: Final = 3

    def __init__(self, difference: ColumnDiff) -> None:
        super().__init__(difference.message)
        self.difference = difference


@dataclass(frozen=True, slots=True)
class ColumnDiff:
    """One difference between a relation's registry spec and its live columns.

    Exactly one of the two types may be absent, and which one decides the verdict:
    a declared column missing live is the additive case S5.1 repairs with
    ``ADD COLUMN``; anything else — a live column the registry does not declare, or
    a type that changed — is breaking.
    """

    relation: str
    column: str
    live_type: str | None
    declared_type: str | None

    @property
    def additive(self) -> bool:
        """True when the repair is an `ADD COLUMN` (S5.1)."""
        return self.live_type is None

    @property
    def breaking(self) -> bool:
        return not self.additive

    @property
    def message(self) -> str:
        """The S5.1 message for a breaking difference, spelled exactly as S5.1 does."""
        declared = self.declared_type if self.declared_type is not None else _UNDECLARED
        return (
            f"{ERR_SCHEMA_BREAKING}: {self.relation}.{self.column}: {self.live_type} -> {declared}"
        )


@dataclass(frozen=True, slots=True)
class RelationResult:
    """What `apply` did to one relation, and the statement it executed doing it.

    ``statement`` is ``None`` for an `exists` relation — the idempotent second run
    of S5.1 executes nothing at all rather than a statement that happens to be a
    no-op — which is also what makes "every statement `apply` executed" an
    inspectable property of its return value.
    """

    relation: str
    action: RelationAction
    statement: str | None


@dataclass(frozen=True, slots=True)
class EvolutionPlan:
    """What S5.1 permits between one relation's live shape and its declared spec.

    The unit of the classification, and the reason :func:`plan_evolution` is pure:
    "is this change additive or breaking" is decidable from two column lists, so it
    is decided where a test can put both of them in front of it rather than inside
    the call that is about to execute DDL against a lake.

    ``differences`` is in the order :func:`plan_evolution` produced it — declared
    columns, then live columns the registry does not declare, then domains — because
    :attr:`breaking` reports the *first* one and two runs against the same lake must
    name the same column.
    """

    relation: str
    differences: tuple[ColumnDiff, ...]

    @property
    def additive(self) -> tuple[ColumnDiff, ...]:
        """The differences an `ALTER TABLE … ADD COLUMN` repairs (S5.1)."""
        return tuple(difference for difference in self.differences if difference.additive)

    @property
    def breaking(self) -> tuple[ColumnDiff, ...]:
        """The differences S5.1 refuses to migrate, in the order they were found."""
        return tuple(difference for difference in self.differences if difference.breaking)

    @property
    def kind(self) -> ChangeKind:
        """The plan's verdict: breaking wins, then additive, then no-op.

        Breaking wins over additive on purpose. A plan holding both — which is what a
        *renamed* column looks like, the old name undeclared and the new one absent —
        must abort rather than add the new column and leave the old one behind.
        """
        if self.breaking:
            return ChangeKind.BREAKING
        if self.additive:
            return ChangeKind.ADD_COLUMN
        return ChangeKind.NOOP

    @property
    def statements(self) -> tuple[str, ...]:
        """The `ADD COLUMN` statements this plan applies, in registry order.

        Raises:
            SchemaBreakingError: the plan is breaking. Returning an empty tuple would
                let a caller execute "nothing" and call the relation reconciled, which
                is the silent destructive migration S5.1 forbids.
        """
        breaking = self.breaking
        if breaking:
            raise SchemaBreakingError(breaking[0])
        return tuple(add_column_statement(difference) for difference in self.additive)


def assert_ddl_owned(relation: str) -> TableSpec:
    """Return ``relation``'s spec, refusing anything `ddl.py` does not own (S5.0).

    Raises:
        OwnershipViolation: if ``relation`` is dbt-owned, or is not a relation of
            S5 at all. An unknown name is the same defect seen earlier: DDL for a
            relation the review-time authority does not declare.
    """
    spec = REGISTRY.get(relation)
    if spec is None:
        raise OwnershipViolation(f"{relation} is not a relation of S5")
    if spec.owner is not Owner.DDL:
        raise OwnershipViolation(f"{relation} is dbt-owned; ddl.py must never issue DDL against it")
    return spec


def create_statement(relation: str) -> str:
    """The `CREATE TABLE IF NOT EXISTS` statement for a `ddl.py`-owned relation.

    The single emitter: :func:`apply` executes exactly what this returns, so the
    ownership guard cannot be bypassed by a second construction path.
    """
    return create_table_sql(assert_ddl_owned(relation))


def add_column_statement(difference: ColumnDiff) -> str:
    """The S5.1 `ADD COLUMN` statement for an additive difference.

    No `NOT NULL` and no `DEFAULT`: DuckLake permits neither on an added column,
    which is why S5.1 makes backfilling the owning stage's job.
    """
    if difference.breaking:
        raise ValueError(f"{difference.message} is not additive")
    assert_ddl_owned(difference.relation)
    return (
        f"ALTER TABLE {SCHEMA_QUALIFIER}.{difference.relation} "
        f"ADD COLUMN {difference.column} {difference.declared_type};"
    )


def live_columns(connection: duckdb.DuckDBPyConnection, relation: str) -> Mapping[str, str]:
    """The live columns of ``relation``, name to type, in catalog order.

    Empty when the relation does not exist — a relation has at least one column,
    so the empty mapping is unambiguous, and it is what lets :func:`apply` treat
    "absent" and "present" as the same question.

    Deliberately not ownership-guarded: reading a dbt-owned relation's live shape
    is how a caller proves `ddl.py` left it alone (S5.0), and reading is not DDL.
    """
    rows = connection.execute(
        "SELECT column_name, data_type FROM duckdb_columns() "
        "WHERE database_name = ? AND schema_name = ? AND table_name = ? "
        "ORDER BY column_index",
        [_DATABASE, _SCHEMA, relation],
    ).fetchall()
    return MappingProxyType({str(name): str(data_type) for name, data_type in rows})


def _catalog_type(declared: str) -> str:
    """The declared S5 type as the catalog views spell it."""
    return _CATALOG_SPELLING.get(declared, declared)


def _relation_diff(spec: TableSpec, live: Mapping[str, str]) -> tuple[ColumnDiff, ...]:
    """Differences for one live relation, declared columns first, then the strays.

    Order is fixed rather than incidental: :func:`evolve` reports the *first*
    breaking difference, so two runs against the same lake must name the same
    column.

    Nullability is not compared. `NOT NULL` is the only constraint DuckLake
    enforces and S5.1's message carries types only; a nullability difference is
    also unrepairable, since an added column may not be `NOT NULL`, so reporting
    one as breaking would abort every run against a lake nothing can fix.
    """
    differences: list[ColumnDiff] = []
    for column in spec.columns:
        live_type = live.get(column.name)
        declared = _catalog_type(column.type)
        if live_type is None:
            differences.append(ColumnDiff(spec.name, column.name, None, column.type))
        elif live_type != declared:
            differences.append(ColumnDiff(spec.name, column.name, live_type, column.type))
    declared_names = frozenset(spec.column_names)
    for name, live_type in live.items():
        if name not in declared_names:
            differences.append(ColumnDiff(spec.name, name, live_type, None))
    return tuple(differences)


def _rendered_domain(members: frozenset[str]) -> str:
    """A domain as the S5.1 message renders it, in a stable order."""
    return _DOMAIN_RENDERING.format(members=", ".join(sorted(members)))


def _domain_diff(
    spec: TableSpec, live_enums: Mapping[str, frozenset[str]]
) -> tuple[ColumnDiff, ...]:
    """The domains that lost a member between ``live_enums`` and ``spec`` (S5.1).

    Only a *removal* is a difference. Adding a member widens the domain, which no
    stored row can violate; removing one orphans every row already carrying it, which
    is why S5.1 lists it beside a dropped column rather than beside an added one.

    A column the live side declares no domain for is not compared: DuckLake has no
    `ENUM` (S5.0), so a domain is a dbt `accepted_values` test and the live catalog
    cannot be asked what vocabulary it was written under. That is exactly why this arm
    is reachable only from a registry pair and never from :func:`preflight_schema`.
    """
    differences: list[ColumnDiff] = []
    for column, declared in spec.enums.items():
        live_domain = live_enums.get(column)
        if live_domain is None or live_domain <= declared:
            continue
        differences.append(
            ColumnDiff(spec.name, column, _rendered_domain(live_domain), _rendered_domain(declared))
        )
    return tuple(differences)


def plan_evolution(live: Mapping[str, str] | TableSpec, spec: TableSpec) -> EvolutionPlan:
    """Classify every difference between a relation's live shape and ``spec`` (S5.1).

    Pure. It opens nothing, executes nothing and reads no catalog, which is what makes
    the additive/breaking classification — the decision that turns a run into an exit
    ``3`` — assertable on a bare runner over hand-built pairs.

    ``live`` takes either shape the two callers have. A :class:`~collections.abc.Mapping`
    of column name to type is what :func:`live_columns` reads off the catalog, and it
    is all a preflight can know. A :class:`~er.lake.model.TableSpec` is the *previous
    registry* — the shape a lake built by an earlier revision of this code has — and it
    additionally carries the `accepted_values` domains, so that arm of S5.1's breaking
    list is decidable at all.

    The four breaking shapes S5.1 names all fall out of the column comparison: a
    **dropped** column is live and undeclared, a **renamed** one is a drop and an add
    together, a **narrowed or changed type** is a declared column whose live type
    differs, and a **removed domain member** is :func:`_domain_diff`'s. Nullability is
    deliberately not compared — see :func:`_relation_diff`.
    """
    live_columns_map = _live_columns_of(live)
    differences = _relation_diff(spec, live_columns_map)
    if isinstance(live, TableSpec):
        differences = (*differences, *_domain_diff(spec, live.enums))
    return EvolutionPlan(relation=spec.name, differences=differences)


def _live_columns_of(live: Mapping[str, str] | TableSpec) -> Mapping[str, str]:
    """``live`` as the name-to-type mapping the column comparison works over."""
    if isinstance(live, TableSpec):
        return MappingProxyType(
            {column.name: _catalog_type(column.type) for column in live.columns}
        )
    return live


def preflight_schema(connection: duckdb.DuckDBPyConnection) -> None:
    """Abort if any live `ddl.py`-owned relation has drifted breakingly (S5.1).

    The entry path S5.1 gives `er init` and *every* stage: it reads, classifies and
    raises, and it never executes a statement — so a lake carrying a breaking
    difference is left exactly as it was found and no snapshot is committed.

    Relations that do not exist yet are not differences, for the reason :func:`diff`
    gives: an uninitialised namespace must be distinguishable from a drifted one, and
    a preflight that refused the former would refuse the very `er init` that repairs it.

    Raises:
        SchemaBreakingError: on the first breaking difference, in registry order.
    """
    for relation in DDL_OWNED:
        live = live_columns(connection, relation)
        if not live:
            continue
        breaking = plan_evolution(live, REGISTRY[relation]).breaking
        if breaking:
            raise SchemaBreakingError(breaking[0])


def diff(connection: duckdb.DuckDBPyConnection) -> tuple[ColumnDiff, ...]:
    """Every difference between the registry and the live `ddl.py`-owned relations.

    Relations that do not exist yet are not differences: :func:`apply` creates them
    whole, and reporting fourteen absent tables as fourteen-times-N additive
    differences would make an uninitialised lake indistinguishable from a drifted
    one.
    """
    differences: list[ColumnDiff] = []
    for relation in DDL_OWNED:
        live = live_columns(connection, relation)
        if not live:
            continue
        differences.extend(plan_evolution(live, REGISTRY[relation]).differences)
    return tuple(differences)


def apply(connection: duckdb.DuckDBPyConnection) -> tuple[RelationResult, ...]:
    """Create every missing `ddl.py`-owned relation; report what each one did.

    Idempotent by S5.1: a relation that already exists is reported ``exists`` and
    nothing is executed against it. Column reconciliation is :func:`evolve`'s job
    and is deliberately a second call — `er init` runs both, while a stage
    preflight that only needs to know the lake is initialised runs neither twice.

    dbt-owned relations are never visited, so a hand-made one with the wrong
    columns is left byte-identical (S5.0).
    """
    results: list[RelationResult] = []
    for relation in DDL_OWNED:
        if live_columns(connection, relation):
            results.append(RelationResult(relation, RelationAction.EXISTS, None))
            continue
        statement = create_statement(relation)
        connection.execute(statement)
        results.append(RelationResult(relation, RelationAction.CREATED, statement))
    return tuple(results)


def evolve(connection: duckdb.DuckDBPyConnection) -> tuple[str, ...]:
    """Reconcile live columns with the registry, additively; report the statements run.

    Every difference is classified before anything executes, so a lake carrying a
    breaking difference is left exactly as it was found: S5.1 requires that no
    snapshot is committed, and a half-applied set of `ADD COLUMN`s would commit
    several.

    Raises:
        SchemaBreakingError: on the first breaking difference, in registry order.
    """
    preflight_schema(connection)
    statements = tuple(add_column_statement(difference) for difference in diff(connection))
    for statement in statements:
        connection.execute(statement)
    return statements
