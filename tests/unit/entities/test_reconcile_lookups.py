"""Bulk key lookups preserve entity expansion and the scope of tombstone removal."""

from collections import defaultdict
from collections.abc import Callable, Iterable, Iterator
from typing import Any

import duckdb
import pytest

from er.entities.cluster import _standardized_records, current_membership
from er.entities.reconcile_stage import _current_partition
from er.entities.retraction import corpus_absent_keys, retract_tombstoned_records


@pytest.fixture
def connection() -> Iterator[duckdb.DuckDBPyConnection]:
    with duckdb.connect() as handle:
        handle.execute("ATTACH ':memory:' AS lake")
        handle.execute(
            "CREATE TABLE lake.main.entity_membership "
            "(source_system VARCHAR, source_record_id VARCHAR, "
            "record_key VARCHAR, entity_id VARCHAR)"
        )
        handle.execute("CREATE TABLE lake.main.int_std_records (record_key VARCHAR)")
        yield handle


def populate(connection: duckdb.DuckDBPyConnection) -> dict[str, str]:
    keys = [f"crm:{index:05d}" for index in range(2054)] + ["crm:O'Brien", "webforms:名字"]
    membership = {key: f"entity'{index // 2}" for index, key in enumerate(keys)}
    connection.execute(
        "INSERT INTO lake.main.entity_membership "
        "SELECT unnest(?::VARCHAR[]), unnest(?::VARCHAR[]), "
        "unnest(?::VARCHAR[]), unnest(?::VARCHAR[])",
        [
            [key.split(":", 1)[0] for key in keys],
            [key.split(":", 1)[1] for key in keys],
            keys,
            list(membership.values()),
        ],
    )
    return membership


def test_membership_expands_whole_entities_from_single_use_iterators(
    connection: duckdb.DuckDBPyConnection,
) -> None:
    membership = populate(connection)
    requested = [*list(membership)[::2], "crm:00000", "crm:missing"]
    assert current_membership(connection, iter(requested)) == membership
    expected: dict[str, set[str]] = defaultdict(set)
    for key, entity in membership.items():
        expected[entity].add(key)
    assert _current_partition(connection, iter(requested)) == expected


def test_corpus_lookup_and_retraction_only_change_requested_tombstones(
    connection: duckdb.DuckDBPyConnection,
) -> None:
    membership = populate(connection)
    keys = list(membership)
    deleted = set(keys[::3]) | {"crm:O'Brien", "webforms:名字"}
    live = set(keys) - deleted
    connection.execute(
        "INSERT INTO lake.main.int_std_records SELECT unnest(?::VARCHAR[])", [sorted(live)]
    )
    requested = [*keys[:-8], "crm:O'Brien", "crm:not-in-corpus", keys[0]]
    assert _standardized_records(connection, iter(requested)) == live.intersection(requested)
    assert corpus_absent_keys(connection, iter(requested)) == set(requested) - live
    removed = deleted.intersection(requested)
    expected: dict[str, list[str]] = defaultdict(list)
    for key in sorted(removed):
        expected[membership[key]].append(key)
    assert retract_tombstoned_records(connection, iter(requested)) == {
        entity: tuple(members) for entity, members in expected.items()
    }
    assert dict(
        connection.execute(
            "SELECT record_key, entity_id FROM lake.main.entity_membership"
        ).fetchall()
    ) == {key: entity for key, entity in membership.items() if key not in removed}


@pytest.mark.parametrize(
    "lookup",
    [
        current_membership,
        _current_partition,
        _standardized_records,
        corpus_absent_keys,
        retract_tombstoned_records,
    ],
)
def test_empty_lookup_does_not_require_lake_relations(
    lookup: Callable[[duckdb.DuckDBPyConnection, Iterable[str]], Any],
) -> None:
    with duckdb.connect() as connection:
        assert not lookup(connection, iter(()))
