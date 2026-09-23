"""Production SQL reconciliation must not inherit cuts from earlier batches."""

from collections.abc import Iterator
from datetime import datetime
from pathlib import Path

import duckdb
import pytest

from er.config.loader import load_config
from er.entities.ids import CountingIdFactory
from er.entities.reconcile_stage import run_reconcile_stage
from er.lake.model import REGISTRY, Owner, create_table_sql
from er.obs.counters import StageCounters
from er.obs.runctx import StageRun


@pytest.fixture
def graph() -> Iterator[duckdb.DuckDBPyConnection]:
    with duckdb.connect() as c:
        c.execute("ATTACH ':memory:' AS lake")
        for relation in REGISTRY.values():
            if relation.owner == Owner.DDL:
                c.execute(create_table_sql(relation))
        c.execute(
            "CREATE TABLE lake.main.int_std_records (record_key VARCHAR, content_hash VARCHAR)"
        )
        c.execute(
            "INSERT INTO lake.main.int_std_records VALUES "
            "('crm:a','h'),('crm:b','h'),('crm:c','h'),('crm:e','h')"
        )
        c.execute(
            "INSERT INTO lake.main.assertions VALUES "
            "('1','crm:a','crm:e','never',true,'test','2026-01-01',NULL,NULL,NULL),"
            "('2','crm:b','crm:c','never',true,'test','2026-01-01',NULL,NULL,NULL)"
        )
        edges(c, [("a", "b", 0.96), ("a", "c", 0.98), ("c", "e", 0.98)])
        yield c


def edges(c: duckdb.DuckDBPyConnection, values: list[tuple[str, str, float]]) -> None:
    c.executemany(
        "INSERT INTO lake.main.match_scores "
        "(rec_a_key,rec_b_key,match_probability,model_version,tf_snapshot_id,"
        "rec_a_content_hash,rec_b_content_hash,evidence,is_active,run_id,scored_at) "
        "VALUES ('crm:' || ?, 'crm:' || ?, ?, 'v1','tf1','h','h','{}',true,'scores','2026-01-01')",
        values,
    )


def reconcile(c: duckdb.DuckDBPyConnection, ids: CountingIdFactory, *, full: bool) -> int:
    cfg = load_config(Path("configs/test.yaml"))
    cfg = cfg.model_copy(
        update={"thresholds": cfg.thresholds.model_copy(update={"auto_merge": 0.95})}
    )
    result = run_reconcile_stage(
        c,
        cfg,
        StageRun(
            run_id="batch",
            stage="reconcile",
            seq=1,
            started_at=datetime(2026, 1, 3),
            counters=StageCounters(()),
        ),
        model_version="v1",
        tf_snapshot_id="tf1",
        id_factory=ids,
        full=full,
    )
    return result.exit_code


def partition(c: duckdb.DuckDBPyConnection) -> set[tuple[str, ...]]:
    return {
        tuple(row[0])
        for row in c.execute(
            "SELECT list(record_key ORDER BY record_key) FROM lake.main.entity_membership "
            "GROUP BY entity_id"
        ).fetchall()
    }


def watermark(c: duckdb.DuckDBPyConnection) -> None:
    c.execute(
        "INSERT INTO lake.main.runs (run_id,tenant,mode,status,started_at,config_hash,"
        "std_version,survivorship_version,code_version) VALUES "
        "('base','test','full','succeeded','2026-01-02','hash','1','1','code')"
    )
    c.execute(
        "INSERT INTO lake.main.run_stages (run_id,stage,seq,status,started_at,counters) "
        "VALUES ('base','reconcile',1,'succeeded','2026-01-02','{}')"
    )


def test_new_paths_rederive_old_cuts_and_replay_is_idempotent(
    graph: duckdb.DuckDBPyConnection,
) -> None:
    c, ids = graph, CountingIdFactory()
    assert reconcile(c, ids, full=True) == 0
    assert partition(c) == {("crm:a", "crm:b"), ("crm:c", "crm:e")}
    watermark(c)
    c.execute("INSERT INTO lake.main.int_std_records VALUES ('crm:f','h')")
    c.execute(
        "INSERT INTO lake.main.ingest_batches VALUES "
        "('delivery','batch','crm','file',1,0,0,0,0,false,'2026-01-03')"
    )
    c.execute(
        "INSERT INTO lake.main.raw_records VALUES "
        "('crm','f','{}','h',false,NULL,'delivery','2026-01-03')"
    )
    edges(c, [("b", "f", 0.98), ("c", "f", 0.98)])
    reconcile(c, ids, full=False)
    expected = {("crm:a", "crm:b"), ("crm:c", "crm:e", "crm:f")}
    assert partition(c) == expected
    before = c.execute("SELECT * FROM lake.main.cut_edges ORDER BY cut_id").fetchall()
    events = c.execute("SELECT * FROM lake.main.entity_events ORDER BY seq").fetchall()
    assert reconcile(c, ids, full=True) == 10
    assert partition(c) == expected
    assert c.execute("SELECT * FROM lake.main.cut_edges ORDER BY cut_id").fetchall() == before
    assert c.execute("SELECT * FROM lake.main.entity_events ORDER BY seq").fetchall() == events
    # Independent fresh full universe, not just a second pass over the same history.
    for table in ("entity_membership", "entities", "entity_events", "cut_edges"):
        c.execute(f"DELETE FROM lake.main.{table}")
    reconcile(c, CountingIdFactory(), full=True)
    assert partition(c) == expected


def test_full_scope_releases_obsolete_cuts_without_delta_seeds(
    graph: duckdb.DuckDBPyConnection,
) -> None:
    c, ids = graph, CountingIdFactory()
    reconcile(c, ids, full=True)
    c.execute("UPDATE lake.main.assertions SET active=false")
    reconcile(c, ids, full=True)
    assert partition(c) == {("crm:a", "crm:b", "crm:c", "crm:e")}
    assert c.execute("SELECT count(*) FROM lake.main.cut_edges WHERE active").fetchone() == (0,)


def test_standalone_full_match_forces_reconcile_without_record_deltas(
    graph: duckdb.DuckDBPyConnection,
) -> None:
    c, ids = graph, CountingIdFactory()
    c.execute("DELETE FROM lake.main.assertions")
    reconcile(c, ids, full=True)
    watermark(c)
    c.execute("UPDATE lake.main.match_scores SET is_active=false")
    c.execute(
        "INSERT INTO lake.main.run_stages (run_id,stage,seq,status,started_at,counters) "
        "VALUES ('replacement','match',1,'succeeded','2026-01-03','{\"mode\":\"full\"}')"
    )
    reconcile(c, ids, full=False)
    assert partition(c) == {("crm:a",), ("crm:b",), ("crm:c",), ("crm:e",)}


def test_failed_apply_rolls_back_membership_and_cut_history(
    graph: duckdb.DuckDBPyConnection, monkeypatch: pytest.MonkeyPatch
) -> None:
    import er.entities.relational_stage as stage

    def fail(*args: object, **kwargs: object) -> int:
        raise RuntimeError("injected apply failure")

    monkeypatch.setattr(stage, "persist_cuts", fail)
    with pytest.raises(RuntimeError, match="injected"):
        reconcile(graph, CountingIdFactory(), full=True)
    for name in ("entity_membership", "entities", "entity_events", "cut_edges"):
        assert graph.execute(f"SELECT count(*) FROM lake.main.{name}").fetchone() == (0,)
