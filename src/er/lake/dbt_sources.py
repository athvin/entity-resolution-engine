"""`dbt/models/sources.yml`, rendered from the S5 registry (DesignDoc.md S5, S5.0, S12).

DuckLake enforces `NOT NULL` and nothing else — no `PRIMARY KEY`, `UNIQUE`,
`FOREIGN KEY`, `CHECK`, index or `ENUM` (S5.0). So every logical key of S5.0's
ownership table is a dbt test or it is nothing, and this module is where the
registry becomes those tests: one source named ``lake`` declaring the fourteen
`ddl.py`-owned relations, `not_null` for every `NOT NULL` column,
`accepted_values` for every ``∈ {…}`` domain, and `unique` /
`dbt_utils.unique_combination_of_columns` — with S5.0's ``where`` filter where the
key is partial — for every logical key.

Three decisions are load-bearing:

* **The document is generated, never hand-maintained.** S5 is the review-time
  authority, and a hand-edited `sources.yml` is a second one. `render_sources_yml`
  is a pure function of the registry and `tests/unit/test_dbt_sources_parity.py`
  asserts byte equality with the committed file, so a column renamed in `model.py`
  and not re-rendered fails the unit gate instead of silently testing a column that
  no longer exists.
* **The key and domain tests carry the ``keys`` tag** (:data:`KEYS_TAG`). T-KEY-1a's
  whole gate is ``dbt test --select tag:keys``, and a selector that matches nothing
  exits 0 and proves nothing (S12 M1). The referential test is tagged ``refs``
  instead (:data:`REFS_TAG`): it is not a key, and letting it into ``tag:keys``
  would let T-KEY-1a fail for a reason that is not a key violation.
* **The file is `sources.yml`, not `schema.yml`.** S12 M1 says "a
  `dbt/models/schema.yml` declaring the `ddl.py`-owned relations as sources"; dbt
  resolves any ``.yml`` under ``models/``, and `schema.yml` is reserved for the
  dbt-owned **model** contracts that arrive with M2's first dbt-owned relation.
  Sources and enforced model contracts are different objects and do not share a file.

Two S5.0 keys cannot be spelled as a generic test and live in ``dbt/tests/``
instead: `model_registry`'s "at most one row with ``status='active'``" — a
:class:`~er.lake.model.LogicalKey` over the empty tuple — and the canonical pair
ordering ``rec_a_key < rec_b_key`` of the four :data:`~er.lake.model.PAIR_RELATIONS`.
Both are singular tests and both carry the ``keys`` tag, which is why the count of
tests the ``keys`` selector matches is the count rendered here *plus* those two.

The module is pure text: it opens no connection, imports no dbt, and executes
nothing.
"""

from __future__ import annotations

from collections.abc import Iterable, Mapping, Sequence
from dataclasses import dataclass
from typing import Final

from er.lake.model import Column, LogicalKey, Owner, TableSpec

__all__ = [
    "ACCEPTED_VALUES_TEST",
    "COMBINATION_TEST",
    "KEYS_TAG",
    "REFS_TAG",
    "RELATIONSHIPS",
    "RELATIONSHIPS_TEST",
    "SOURCE_DATABASE",
    "SOURCE_NAME",
    "SOURCE_SCHEMA",
    "SOURCES_YML",
    "UNIQUE_TEST",
    "Relationship",
    "render_sources_yml",
]

#: The dbt source every `ddl.py`-owned relation is declared under. One source, not
#: fourteen: `source('lake', <relation>)` then reads exactly like the `lake.main.…`
#: qualification S5 uses everywhere else.
SOURCE_NAME: Final[str] = "lake"

# S5: every relation lives in the attached catalog under alias `lake`, schema
# `main`. Spelled out rather than split from `model.SCHEMA_QUALIFIER` because dbt
# wants the two halves in two keys, and a `.split('.')` here would silently produce
# an empty schema the first time the qualifier gained a third component.
SOURCE_DATABASE: Final[str] = "lake"
SOURCE_SCHEMA: Final[str] = "main"

#: Where the rendered document is committed, relative to the repository root.
SOURCES_YML: Final[str] = "dbt/models/sources.yml"

#: T-KEY-1a's selector (S12 M1). Every logical-key and domain test carries it.
KEYS_TAG: Final[str] = "keys"

#: The referential tests. Deliberately *not* :data:`KEYS_TAG`: a dangling reference
#: is not a key violation, and T-KEY-1a asserts on a key violation.
REFS_TAG: Final[str] = "refs"

# The generic tests S5.0's "Enforced by" column names, under the spellings dbt and
# dbt_utils register them. `dbt_utils` is pinned exactly in `dbt/packages.yml`
# because these strings are what nine of the logical keys mean.
UNIQUE_TEST: Final[str] = "unique"
NOT_NULL_TEST: Final[str] = "not_null"
ACCEPTED_VALUES_TEST: Final[str] = "accepted_values"
RELATIONSHIPS_TEST: Final[str] = "relationships"
COMBINATION_TEST: Final[str] = "dbt_utils.unique_combination_of_columns"


@dataclass(frozen=True, slots=True)
class Relationship:
    """A referential expectation between two `ddl.py`-owned relations.

    Not part of :class:`~er.lake.model.TableSpec`: S5.0's key model is about
    identity, and DuckLake has no `FOREIGN KEY` for this to be the logical form of.
    It is declared here because dbt's `relationships` test is the only place the
    expectation can be executable at all.
    """

    relation: str
    column: str
    to: str
    field: str


#: M3 is one membership row per record, and it is only meaningful if the entity that
#: row names exists. T-INV-1 asserts the stronger form — every
#: `entity_membership.entity_id` has `entities.status='active'` — but that needs a
#: run; this is the arm that holds on any lake.
RELATIONSHIPS: Final[tuple[Relationship, ...]] = (
    Relationship(
        relation="entity_membership", column="entity_id", to="entities", field="entity_id"
    ),
)

# The generated document says so in its first line. A reader who opens
# `dbt/models/sources.yml` looking for the column list must be sent to S5 and to the
# renderer, not left to edit it and wonder why the unit gate went red.
_HEADER: Final[tuple[str, ...]] = (
    "# GENERATED FILE -- do not edit by hand.",
    "#",
    "# Rendered from the S5 TableSpec registry by",
    "# `er.lake.dbt_sources.render_sources_yml`. `tests/unit/test_dbt_sources_parity.py`",
    "# asserts byte equality with that renderer, so an edit here is a second authority",
    "# for S5 and fails the unit gate rather than drifting silently.",
    "#",
    "# DuckLake enforces NOT NULL and nothing else (S5.0), so every logical key below is",
    "# a test or it is nothing. The `keys` tag is what T-KEY-1a's",
    "# `dbt test --select tag:keys` selects.",
)

# Two spaces per level, and the block below is emitted at fixed depths rather than
# through a general YAML writer: the file is compared byte for byte, so the layout is
# part of the contract and a serialiser's formatting choices would be too.
_INDENT: Final[str] = "  "
_TABLE_DEPTH: Final[int] = 3
_COLUMN_DEPTH: Final[int] = 5


def _pad(depth: int) -> str:
    return _INDENT * depth


def _single(value: str) -> str:
    """A single-quoted YAML scalar, doubling embedded quotes."""
    return "'" + value.replace("'", "''") + "'"


def _double(value: str) -> str:
    """A double-quoted YAML scalar.

    Used only for S5.0's ``where`` filters, which carry single quotes of their own
    (``status='open'``) and would otherwise need doubling inside a single-quoted
    scalar — unreadable next to the spec text it is supposed to reproduce.
    """
    return '"' + value.replace("\\", "\\\\").replace('"', '\\"') + '"'


def _flow_list(values: Sequence[str]) -> str:
    """A flow-style sequence of quoted scalars: ``['a', 'b']``."""
    return "[" + ", ".join(_single(value) for value in values) + "]"


def _test_block(
    depth: int,
    name: str,
    *,
    arguments: Mapping[str, str] | None = None,
    tags: Sequence[str] = (),
    where: str | None = None,
) -> list[str]:
    """One entry of a ``data_tests:`` list.

    ``arguments:`` rather than top-level keyword keys because `dbt-core==1.12.2`
    defaults ``require_generic_test_arguments_property`` to true; the flat form
    parses but emits a deprecation on every invocation.
    """
    if not arguments and not tags and where is None:
        return [f"{_pad(depth)}- {name}"]
    lines = [f"{_pad(depth)}- {name}:"]
    # Two levels, not one: `- ` already consumes the first, so a mapping nested under
    # the test name has to clear the name itself, not just the sequence dash.
    body = _pad(depth + 2)
    if arguments:
        lines.append(f"{body}arguments:")
        lines.extend(f"{body}{_INDENT}{key}: {value}" for key, value in arguments.items())
    if tags or where is not None:
        lines.append(f"{body}config:")
        if tags:
            lines.append(f"{body}{_INDENT}tags: {_flow_list(tags)}")
        if where is not None:
            lines.append(f"{body}{_INDENT}where: {_double(where)}")
    return lines


def _column_key(spec: TableSpec, column: str) -> LogicalKey | None:
    """The single-column logical key on ``column``, if S5.0 declares one.

    Single-column keys become `unique` on the column and multi-column keys become
    `dbt_utils.unique_combination_of_columns` on the table, which is exactly the
    split S5.0's "Enforced by" column makes.
    """
    for key in spec.keys:
        if key.columns == (column,):
            return key
    return None


def _relationships_for(spec: TableSpec, column: str) -> tuple[Relationship, ...]:
    return tuple(
        edge for edge in RELATIONSHIPS if edge.relation == spec.name and edge.column == column
    )


def _column_tests(spec: TableSpec, column: Column) -> list[list[str]]:
    """Every test attached to one column, in a fixed order."""
    depth = _COLUMN_DEPTH + 2
    blocks: list[list[str]] = []
    if not column.nullable:
        blocks.append(_test_block(depth, NOT_NULL_TEST))
    key = _column_key(spec, column.name)
    if key is not None:
        blocks.append(_test_block(depth, UNIQUE_TEST, tags=(KEYS_TAG,), where=key.where))
    domain = spec.enums.get(column.name)
    if domain is not None:
        blocks.append(
            _test_block(
                depth,
                ACCEPTED_VALUES_TEST,
                # Sorted because a `frozenset` has no order and the document is
                # compared byte for byte.
                arguments={"values": _flow_list(sorted(domain))},
                tags=(KEYS_TAG,),
            )
        )
    for edge in _relationships_for(spec, column.name):
        blocks.append(
            _test_block(
                depth,
                RELATIONSHIPS_TEST,
                arguments={
                    "to": _double(f"source('{SOURCE_NAME}', '{edge.to}')"),
                    "field": edge.field,
                },
                tags=(REFS_TAG,),
            )
        )
    return blocks


def _column_block(spec: TableSpec, column: Column) -> list[str]:
    lines = [f"{_pad(_COLUMN_DEPTH)}- name: {column.name}"]
    tests = _column_tests(spec, column)
    if tests:
        lines.append(f"{_pad(_COLUMN_DEPTH + 1)}data_tests:")
        for block in tests:
            lines.extend(block)
    return lines


def _table_tests(spec: TableSpec) -> list[list[str]]:
    """The table-level tests: every multi-column logical key of S5.0.

    A key over the empty tuple is deliberately skipped. It is `model_registry`'s "at
    most one row with ``status='active'``", which is uniqueness over no columns under
    a filter — a claim no generic test can express, and the one S5.0 pairs with a
    singular test (``dbt/tests/assert_single_active_model.sql``).
    """
    depth = _TABLE_DEPTH + 2
    return [
        _test_block(
            depth,
            COMBINATION_TEST,
            arguments={"combination_of_columns": _flow_list(key.columns)},
            tags=(KEYS_TAG,),
            where=key.where,
        )
        for key in spec.keys
        if len(key.columns) > 1
    ]


def _table_block(spec: TableSpec) -> list[str]:
    lines = [f"{_pad(_TABLE_DEPTH)}- name: {spec.name}", f"{_pad(_TABLE_DEPTH + 1)}columns:"]
    for column in spec.columns:
        lines.extend(_column_block(spec, column))
    tests = _table_tests(spec)
    if tests:
        lines.append(f"{_pad(_TABLE_DEPTH + 1)}data_tests:")
        for block in tests:
            lines.extend(block)
    return lines


def render_sources_yml(specs: Iterable[TableSpec]) -> str:
    """The whole `dbt/models/sources.yml` document for the `ddl.py`-owned relations.

    dbt-owned specs are skipped rather than rejected: the caller passes the registry,
    which holds both owners, and declaring a dbt-owned relation as a *source* would
    tell dbt that a relation it materializes under an enforced contract is an input
    it does not build (S5.0, D14).

    Args:
        specs: the S5 relation specs, in the order they should be declared —
            :data:`~er.lake.model.REGISTRY` values, which is S5 declaration order.

    Returns:
        The document, newline-terminated, byte-identical to the committed file.
    """
    lines = [
        *_HEADER,
        "version: 2",
        "",
        "sources:",
        f"{_pad(1)}- name: {SOURCE_NAME}",
        f"{_pad(2)}database: {SOURCE_DATABASE}",
        f"{_pad(2)}schema: {SOURCE_SCHEMA}",
        f"{_pad(2)}tables:",
    ]
    for spec in specs:
        if spec.owner is not Owner.DDL:
            continue
        lines.extend(_table_block(spec))
    return "\n".join(lines) + "\n"
