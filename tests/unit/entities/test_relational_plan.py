"""SQL lifecycle planning agrees with the independent pure overlap/event oracle."""

import json
import random
from contextlib import ExitStack

import duckdb
import pytest

from er.entities.ids import CountingIdFactory
from er.entities.reconcile import reconcile_plan
from er.entities.relational import lifecycle_plan
from er.lake.bulk import staged_rows


@pytest.mark.parametrize("seed", range(40))
def test_partition_changes_preserve_assignments_transitions_and_ordered_events(seed: int) -> None:
    randomizer = random.Random(seed)
    keys = [f"crm:{i}" for i in range(30)] + ["web:名字", "crm:O'Brien", "web:e\u0301"]
    old: dict[str, set[str]] = {f"old{i}": set() for i in range(8)}
    grouped: dict[int, set[str]] = {i: set() for i in range(10)}
    for key in keys:
        if randomizer.random() < 0.8:
            old[randomizer.choice(list(old))].add(key)
        if randomizer.random() < 0.8:
            grouped[randomizer.choice(list(grouped))].add(key)
    # Includes orphans, empty old entities, ties, merges, splits and new records.
    groups = [group for group in grouped.values() if group]
    expected = reconcile_plan(old, groups, CountingIdFactory())
    with duckdb.connect() as connection, ExitStack() as stack:
        prior = stack.enter_context(
            staged_rows(
                connection,
                (("record_key", "VARCHAR"), ("entity_id", "VARCHAR")),
                ((key, entity) for entity, members in old.items() for key in members),
            )
        )
        entities = stack.enter_context(
            staged_rows(
                connection,
                (("entity_id", "VARCHAR"),),
                ((entity,) for entity in old),
            )
        )
        labels = stack.enter_context(
            staged_rows(
                connection,
                (("record_key", "VARCHAR"), ("label", "VARCHAR")),
                ((key, min(group)) for group in groups for key in group),
            )
        )
        plan = stack.enter_context(
            lifecycle_plan(
                connection,
                prior,
                entities,
                labels,
                CountingIdFactory(),
            )
        )
        assert connection.execute(
            f"SELECT record_key, entity_id FROM {plan.membership} ORDER BY record_key"
        ).fetchall() == [(a.record_key, a.entity_id) for a in expected.assignments]
        assert connection.execute(
            f"SELECT entity_id, status, merged_into FROM {plan.entities} ORDER BY entity_id"
        ).fetchall() == [(a.entity_id, a.status, a.merged_into) for a in expected.transitions]
        actual = [
            (entity, kind, json.loads(details))
            for entity, kind, details in connection.execute(
                f"SELECT entity_id, event_type, details FROM {plan.events} ORDER BY position"
            ).fetchall()
        ]
        assert actual == [(e.entity_id, e.event_type, e.details) for e in expected.events]
