"""`dbt/models/sources.yml` is generated output, and this is what keeps it that way.

S5 is the review-time authority for every relation, and DuckLake enforces `NOT NULL`
and nothing else (S5.0) — so the logical keys are dbt tests, and a hand-maintained
`sources.yml` would be a second authority for the column lists, the nullability and
the enum domains that those tests are written against. The parity test below asserts
byte equality between the committed document and
:func:`~er.lake.dbt_sources.render_sources_yml` over the registry, which makes any
edit to either side without the other a red unit gate rather than a silent drift.

The remaining three tests assert the *coverage* claims byte equality cannot make:
that the declared table set is exactly the `ddl.py`-owned partition of the registry
in both directions, that every ``∈ {…}`` domain of S5 has an `accepted_values` test,
and that every S5.0 logical key has exactly one test carrying T-KEY-1a's ``keys``
tag. Byte equality would keep passing if the renderer emitted no tests at all.
"""

from __future__ import annotations

import dataclasses
from collections.abc import Mapping, Sequence
from pathlib import Path
from typing import Any

import yaml

from er.lake.dbt_sources import (
    ACCEPTED_VALUES_TEST,
    COMBINATION_TEST,
    KEYS_TAG,
    SOURCE_DATABASE,
    SOURCE_NAME,
    SOURCE_SCHEMA,
    SOURCES_YML,
    UNIQUE_TEST,
    render_sources_yml,
)
from er.lake.model import DBT_OWNED, DDL_OWNED, PAIR_RELATIONS, REGISTRY, Owner

REPO_ROOT = Path(__file__).resolve().parents[2]
COMMITTED = REPO_ROOT / SOURCES_YML
SINGULAR_TESTS = REPO_ROOT / "dbt" / "tests"

# S5.0's ownership table lists exactly this many `ddl.py`-owned relations. A literal,
# not `len(DDL_OWNED)`: a count derived from the registry would agree with it no
# matter what the registry said, which is the drift AC2 is about.
DDL_OWNED_COUNT = 14

# The two S5.0 keys no generic test can express, and the files that carry them.
# `model_registry`'s at-most-one-active row is uniqueness over the empty tuple, and
# the pair ordering is an inequality across four relations at once.
PAIR_ORDERING_TEST = SINGULAR_TESTS / "assert_canonical_pair_ordering.sql"
SINGLE_ACTIVE_TEST = SINGULAR_TESTS / "assert_single_active_model.sql"

# How a singular test declares T-KEY-1a's tag. Spelled as it appears in the file,
# because that literal is what `dbt test --select tag:keys` resolves.
SINGULAR_KEYS_CONFIG = f"config(tags=['{KEYS_TAG}'])"


def _document() -> dict[str, Any]:
    parsed = yaml.safe_load(COMMITTED.read_text(encoding="utf-8"))
    assert isinstance(parsed, dict)
    return parsed


def _tables() -> dict[str, dict[str, Any]]:
    """The committed document's tables, keyed by relation name."""
    sources = _document()["sources"]
    assert len(sources) == 1, "S5 has one namespace, so the document has one source"
    assert sources[0]["name"] == SOURCE_NAME
    assert sources[0]["database"] == SOURCE_DATABASE
    assert sources[0]["schema"] == SOURCE_SCHEMA
    return {table["name"]: table for table in sources[0]["tables"]}


def _declared_tests(entries: Sequence[Any] | None) -> list[tuple[str, Mapping[str, Any]]]:
    """One ``data_tests:`` list as ``(test name, body)`` pairs.

    A bare `not_null` is a string and a configured test is a single-key mapping;
    both are normalised here so the assertions below read the same for either.
    """
    declared: list[tuple[str, Mapping[str, Any]]] = []
    for entry in entries or ():
        if isinstance(entry, str):
            declared.append((entry, {}))
            continue
        assert len(entry) == 1, f"a data_tests entry names one test: {entry}"
        ((name, body),) = entry.items()
        declared.append((name, body or {}))
    return declared


def _column_tests(table: Mapping[str, Any], column: str) -> list[tuple[str, Mapping[str, Any]]]:
    for declared in table["columns"]:
        if declared["name"] == column:
            return _declared_tests(declared.get("data_tests"))
    raise AssertionError(f"{table['name']}.{column} is not declared in {SOURCES_YML}")


def _tagged(body: Mapping[str, Any]) -> list[str]:
    config = body.get("config") or {}
    tags = config.get("tags") or []
    assert isinstance(tags, list)
    return [str(tag) for tag in tags]


def test_rendered_document_equals_committed_file() -> None:
    """AC1: the committed file is exactly what the registry renders — no more, no less."""
    assert COMMITTED.read_text(encoding="utf-8") == render_sources_yml(REGISTRY.values())

    # And the comparison is sensitive: renaming one column of one relation changes
    # the document. Without this, byte equality would still hold if the renderer
    # ignored its argument and emitted a constant.
    raw_records = REGISTRY["raw_records"]
    payload, rest = raw_records.columns[2], raw_records.columns[3:]
    renamed = dataclasses.replace(
        raw_records,
        columns=(*raw_records.columns[:2], dataclasses.replace(payload, name="body"), *rest),
    )
    mutated = tuple(renamed if spec.name == "raw_records" else spec for spec in REGISTRY.values())
    assert render_sources_yml(mutated) != render_sources_yml(REGISTRY.values())


def test_declared_tables_equal_ddl_owned_registry() -> None:
    """AC2: the declared set is the `ddl.py`-owned partition, asserted in both directions."""
    declared = set(_tables())
    assert declared == set(DDL_OWNED)
    assert len(declared) == DDL_OWNED_COUNT
    # The other direction of D14: a dbt-owned relation is materialized under an
    # enforced contract, so declaring one as a *source* would tell dbt that a
    # relation it builds is an input it does not.
    assert declared.isdisjoint(DBT_OWNED)

    for name, table in _tables().items():
        spec = REGISTRY[name]
        assert spec.owner is Owner.DDL
        assert [column["name"] for column in table["columns"]] == list(spec.column_names)


def test_every_enum_domain_has_accepted_values_test() -> None:
    """AC7: every ``∈ {…}`` domain of the S5 DDL is validated by a tagged test (S5.0)."""
    tables = _tables()
    found: set[tuple[str, str]] = set()
    for name, table in tables.items():
        for column, domain in REGISTRY[name].enums.items():
            tests = dict(_column_tests(table, column))
            assert ACCEPTED_VALUES_TEST in tests, f"{name}.{column} has no domain test"
            body = tests[ACCEPTED_VALUES_TEST]
            assert body["arguments"]["values"] == sorted(domain)
            assert KEYS_TAG in _tagged(body)
            found.add((name, column))

    # In the other direction: no relation carries a domain test for a column S5
    # declares no vocabulary for, which would be a vocabulary living only here.
    for name, table in tables.items():
        for declared in table["columns"]:
            if ACCEPTED_VALUES_TEST in dict(_declared_tests(declared.get("data_tests"))):
                assert (name, declared["name"]) in found


def test_every_s5_0_logical_key_has_a_tagged_test() -> None:
    """AC3: every logical key of every `ddl.py`-owned relation is exactly one tagged test."""
    tables = _tables()
    for name, table in tables.items():
        table_tests = _declared_tests(table.get("data_tests"))
        combinations = [body for test, body in table_tests if test == COMBINATION_TEST]
        multi_column = [key for key in REGISTRY[name].keys if len(key.columns) > 1]
        # Counted, not just looked up: `run_stages` carries two multi-column keys,
        # and a renderer that emitted one of them would still satisfy a search.
        assert len(combinations) == len(multi_column)
        for key in REGISTRY[name].keys:
            if len(key.columns) == 1:
                bodies = [
                    body
                    for test, body in _column_tests(table, key.columns[0])
                    if test == UNIQUE_TEST
                ]
            elif key.columns:
                bodies = [
                    body
                    for body in combinations
                    if body["arguments"]["combination_of_columns"] == list(key.columns)
                ]
            else:
                # Uniqueness over no columns under a filter: the at-most-one-active
                # row of `model_registry`, which only a singular test can express.
                assert name == "model_registry"
                continue
            assert len(bodies) == 1, f"{name} {key.columns}: expected exactly one test"
            assert KEYS_TAG in _tagged(bodies[0])
            # A partial key without its filter is a *different*, stronger claim, and
            # it would fail the moment a second retracted assertion existed (M6).
            assert (bodies[0].get("config") or {}).get("where") == key.where

    # The two singular tests complete the coverage, and both must be selectable by
    # T-KEY-1a: a `keys` selector that misses them exits 0 on a lake that violates
    # the very invariants they hold.
    for path in (PAIR_ORDERING_TEST, SINGLE_ACTIVE_TEST):
        assert SINGULAR_KEYS_CONFIG in path.read_text(encoding="utf-8")
    ordering = PAIR_ORDERING_TEST.read_text(encoding="utf-8")
    for relation in PAIR_RELATIONS:
        assert f"'{SOURCE_NAME}', '{relation}'" in ordering
