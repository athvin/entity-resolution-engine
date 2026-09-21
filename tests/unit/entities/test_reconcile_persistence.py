"""Large plans retain lifecycle and event semantics across staging batches."""

from datetime import datetime

import duckdb

from er.entities.ids import CountingIdFactory
from er.entities.reconcile import reconcile_plan
from er.entities.reconcile_stage import apply_reconcile_plan
from er.lake.bulk import BATCH_ROWS
from er.lake.model import REGISTRY, create_table_sql


def test_batched_plan_insert_retry_and_merge_preserve_memberships_and_events() -> None:
    with duckdb.connect() as connection:
        connection.execute("ATTACH ':memory:' AS lake")
        for name in ("entities", "entity_membership", "entity_events"):
            connection.execute(create_table_sql(REGISTRY[name]))
        ids = CountingIdFactory()
        groups = [frozenset((f"crm:{i:06}",)) for i in range(2 * BATCH_ROWS + 3)]
        plan = reconcile_plan({}, groups, ids)
        stamp = datetime(2026, 1, 1)
        assert apply_reconcile_plan(
            connection, plan, run_id="run1", occurred_at=stamp, ids=ids
        ) == len(groups)
        expected = {row.record_key: row.entity_id for row in plan.assignments}
        assert (
            dict(
                connection.execute(
                    "SELECT record_key, entity_id FROM lake.main.entity_membership"
                ).fetchall()
            )
            == expected
        )
        original_events = connection.execute(
            "SELECT * FROM lake.main.entity_events ORDER BY seq"
        ).fetchall()
        assert (
            apply_reconcile_plan(connection, plan, run_id="run1", occurred_at=stamp, ids=ids) == 0
        )
        assert (
            connection.execute("SELECT * FROM lake.main.entity_events ORDER BY seq").fetchall()
            == original_events
        )

        old = {entity: frozenset((record,)) for record, entity in expected.items()}
        merged = reconcile_plan(old, [groups[0] | groups[1], *groups[2:]], ids)
        expected.update({row.record_key: row.entity_id for row in merged.assignments})
        assert apply_reconcile_plan(
            connection, merged, run_id="run2", occurred_at=datetime(2026, 1, 2), ids=ids
        ) == len(merged.events)
        assert (
            dict(
                connection.execute(
                    "SELECT record_key, entity_id FROM lake.main.entity_membership"
                ).fetchall()
            )
            == expected
        )
        assert connection.execute(
            "SELECT count(*) FROM lake.main.entities WHERE created_at <> ?", [stamp]
        ).fetchone() == (0,)
        assert connection.execute(
            "SELECT count(*) FROM lake.main.entities WHERE status = 'merged'"
        ).fetchone() == (1,)
        assert not connection.execute(
            "SELECT table_name FROM duckdb_tables() WHERE table_name LIKE 'er_bulk_%'"
        ).fetchall()
