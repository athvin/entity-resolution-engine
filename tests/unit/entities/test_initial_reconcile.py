"""The bounded first-load path is the ordinary lifecycle plan, with local staging."""

import json
from collections.abc import Iterator
from datetime import datetime
from pathlib import Path

import duckdb
import pytest

from er.entities.cluster import label_propagate, label_propagate_relations
from er.entities.events import append_events
from er.entities.ids import CountingIdFactory
from er.entities.initial import initial_load_eligible, prepare_initial_plan
from er.entities.reconcile import reconcile_plan
from er.entities.reconcile_stage import (
    _merge_entity_relation,
    _merge_membership_relation,
    apply_reconcile_plan,
)
from er.errors import NonConvergenceError
from er.lake.bulk import BATCH_ROWS
from er.lake.model import REGISTRY, create_table_sql

STAMP = datetime(2026, 1, 1)


@pytest.fixture
def connection() -> Iterator[duckdb.DuckDBPyConnection]:
    with duckdb.connect() as c:
        c.execute("ATTACH ':memory:' AS lake")
        for name in ("entities", "entity_membership", "entity_events"):
            c.execute(create_table_sql(REGISTRY[name]))
        c.execute("CREATE TABLE lake.main.assertions (assertion_id VARCHAR)")
        c.execute("CREATE TABLE lake.main.cut_edges (cut_id VARCHAR)")
        c.execute("CREATE TABLE lake.main.review_queue (resolved_at TIMESTAMP)")
        c.execute("CREATE TABLE lake.main.int_std_records (record_key VARCHAR)")
        c.execute("CREATE TABLE lake.main.ingest_batches (ingest_batch_id VARCHAR, run_id VARCHAR)")
        c.execute("INSERT INTO lake.main.ingest_batches VALUES ('batch', 'run')")
        c.execute(
            "CREATE TABLE lake.main.raw_records (source_system VARCHAR, source_record_id VARCHAR, "
            "ingest_batch_id VARCHAR, is_deleted BOOLEAN)"
        )
        c.execute(
            "CREATE TABLE lake.main.match_scores (rec_a_key VARCHAR, rec_b_key VARCHAR, "
            "match_probability DOUBLE, model_version VARCHAR, tf_snapshot_id VARCHAR, "
            "is_active BOOLEAN, scored_at TIMESTAMP, run_id VARCHAR)"
        )
        yield c


def populate(c: duckdb.DuckDBPyConnection, groups: list[list[str]]) -> None:
    nodes = [key for group in groups for key in group]
    c.execute("INSERT INTO lake.main.int_std_records SELECT unnest(?::VARCHAR[])", [nodes])
    c.execute(
        "INSERT INTO lake.main.raw_records SELECT split_part(key, ':', 1), "
        "split_part(key, ':', 2), 'batch', false FROM (SELECT unnest(?::VARCHAR[]) AS key)",
        [nodes],
    )
    edges = [
        tuple(sorted((a, b))) for group in groups for a, b in zip(group, group[1:], strict=False)
    ]
    if edges:
        c.execute(
            "INSERT INTO lake.main.match_scores SELECT unnest(?::VARCHAR[]), "
            "unnest(?::VARCHAR[]), 0.9, 'v1', 'tf1', true, TIMESTAMP '2026-01-01', 'run'",
            [[edge[0] for edge in edges], [edge[1] for edge in edges]],
        )


def assert_clean(c: duckdb.DuckDBPyConnection) -> None:
    assert not c.execute(
        "SELECT table_name FROM duckdb_tables() WHERE temporary "
        "AND (table_name LIKE 'er_bulk_%' OR table_name LIKE 'er_label_prop_%')"
    ).fetchall()
    assert not c.execute(
        "SELECT view_name FROM duckdb_views() WHERE view_name LIKE 'er_initial_edges_%'"
    ).fetchall()


@pytest.mark.parametrize("reason", [None, "correction_pass"])
def test_initial_plan_matches_complete_plan_across_batches(
    connection: duckdb.DuckDBPyConnection, reason: str | None
) -> None:
    c = connection
    groups = [[f"crm:{i:06}a", f"web:{i:06}b"] for i in range(BATCH_ROWS + 3)]
    groups += [[f"crm:large{i:06}" for i in range(BATCH_ROWS + 2)], ["crm:O'Brien"], ["web:名字"]]
    populate(c, groups)
    assert initial_load_eligible(c, "run")
    ids = CountingIdFactory()
    c.execute("BEGIN")
    with prepare_initial_plan(
        c,
        run_id="run",
        model_version="v1",
        tf_snapshot_id="tf1",
        auto_merge=0.9,
        max_iterations=50,
        ids=ids,
        reason=reason,
    ) as prepared:
        assert prepared is not None
        assert prepared.clusters == len(groups)
        assert prepared.records == sum(map(len, groups))
        _merge_entity_relation(c, prepared.entities, STAMP, "run")
        _merge_membership_relation(c, prepared.membership, STAMP, "run")
        assert append_events(c, prepared.events, occurred_at=STAMP) == len(groups)
    actual = {
        name: c.execute(f"SELECT * FROM lake.main.{name} ORDER BY ALL").fetchall()
        for name in ("entities", "entity_membership", "entity_events")
    }
    assert_clean(c)
    c.execute("ROLLBACK")
    assert c.execute("SELECT count(*) FROM lake.main.entity_membership").fetchone() == (0,)
    ids = CountingIdFactory()
    plan = reconcile_plan({}, groups, ids)
    apply_reconcile_plan(c, plan, run_id="run", occurred_at=STAMP, ids=ids, reason=reason)
    for name, rows in actual.items():
        assert c.execute(f"SELECT * FROM lake.main.{name} ORDER BY ALL").fetchall() == rows
    assert not initial_load_eligible(c, "run")


@pytest.mark.parametrize(
    "change",
    [
        "UPDATE lake.main.ingest_batches SET run_id='different'",
        "INSERT INTO lake.main.assertions VALUES ('prior')",
        "INSERT INTO lake.main.cut_edges VALUES ('prior')",
        "INSERT INTO lake.main.int_std_records VALUES ('crm:undelivered')",
        "INSERT INTO lake.main.raw_records VALUES ('crm','absent','batch',false)",
        "UPDATE lake.main.raw_records SET is_deleted=true",
        "INSERT INTO lake.main.review_queue VALUES (TIMESTAMP '2026-01-01')",
        "UPDATE lake.main.match_scores SET rec_b_key='web:absent'",
    ],
)
def test_initial_path_declines_partial_deliveries_and_history(
    connection: duckdb.DuckDBPyConnection, change: str
) -> None:
    populate(connection, [["crm:a", "web:b"]])
    connection.execute(change)
    with prepare_initial_plan(
        connection,
        run_id="run",
        model_version="v1",
        tf_snapshot_id="tf1",
        auto_merge=0.9,
        max_iterations=50,
        ids=CountingIdFactory(),
    ) as prepared:
        assert prepared is None
    assert_clean(connection)


def test_nonconvergence_leaves_no_lake_writes_or_scratch(
    connection: duckdb.DuckDBPyConnection,
) -> None:
    populate(connection, [["crm:a", "crm:b", "crm:c", "crm:d"]])
    with pytest.raises(NonConvergenceError, match="No membership was written"):
        with prepare_initial_plan(
            connection,
            run_id="run",
            model_version="v1",
            tf_snapshot_id="tf1",
            auto_merge=0.9,
            max_iterations=1,
            ids=CountingIdFactory(),
        ):
            pytest.fail("non-convergent plan was yielded")
    for name in ("entities", "entity_membership", "entity_events"):
        assert connection.execute(f"SELECT count(*) FROM lake.main.{name}").fetchone() == (0,)
    assert_clean(connection)


def test_relation_clustering_matches_existing_loop(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    nodes = [f"crm:{i:04}" for i in range(130)]
    pairs = list(zip(nodes[:126], nodes[1:127], strict=True))
    with duckdb.connect() as c:
        expected = label_propagate(c, nodes, pairs, max_iterations=50)
        c.execute("CREATE TABLE nodes AS SELECT unnest(?::VARCHAR[]) AS record_key", [nodes])
        c.execute(
            "CREATE TABLE edges AS SELECT unnest(?::VARCHAR[]) AS rec_a_key, "
            "unnest(?::VARCHAR[]) AS rec_b_key",
            [[a for a, _ in pairs], [b for _, b in pairs]],
        )
        monkeypatch.setenv("ER_PROFILE_DIR", str(tmp_path))
        with label_propagate_relations(c, "nodes", "edges", max_iterations=50) as (
            relation,
            rounds,
        ):
            assert (
                dict(c.execute(f"SELECT record_key,label FROM {relation}").fetchall())
                == expected.labels
            )
            assert rounds == expected.iterations
            # The clustering span ends before callers consume the yielded relation.
            spans = [
                json.loads(line)
                for path in tmp_path.glob("events-*.jsonl")
                for line in path.read_text().splitlines()
            ]
            completed = [
                event
                for event in spans
                if event["name"] == "reconcile.label_propagation" and event["event"] == "span_end"
            ]
            assert len(completed) == 1
            assert completed[0]["status"] == "succeeded"
            assert completed[0]["metrics"]["rows_out"] == len(nodes)
        assert_clean(c)
