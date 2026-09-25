"""Metadata changes refresh golden rows without requiring membership events."""

import json
from datetime import datetime
from pathlib import Path

import duckdb
import pytest

from er.config.loader import load_config
from er.dbt_runner import DbtResult
from er.errors import StageFailure
from er.golden.assemble import assemble, compute_touched_set, materialize_touched_entities
from er.lake.model import REGISTRY
from er.obs.counters import DECLARED_COUNTERS, StageCounters
from er.obs.runctx import held


def test_metadata_is_excluded_from_every_matching_projection():
    from er.matching import full, incremental, train

    with duckdb.connect() as connection:
        connection.execute("ATTACH ':memory:' AS lake")
        projections = [
            f"CAST(NULL AS {column.type.replace('LIST(VARCHAR)', 'VARCHAR[]')}) AS {column.name}"
            for column in REGISTRY["int_std_records"].columns
        ]
        connection.execute(
            "CREATE TABLE lake.main.int_std_records AS SELECT " + ", ".join(projections)
        )
        connection.execute(
            "UPDATE lake.main.int_std_records SET record_key='crm:a', "
            'metadata=\'{"client_only":"keep out of Splink"}\''
        )
        assert full._materialize_corpus(connection) == 1
        assert train._materialize_corpus(connection) == 1
        connection.execute(incremental._BATCH_KEYS_DDL)
        connection.execute(incremental._BATCH_KEYS_INSERT, [["crm:a"]])
        connection.execute(incremental._BATCH_SQL)
        connection.execute(incremental._PRIOR_CORPUS_SQL)
        for relation in (
            full.MATCH_CORPUS_RELATION,
            train.TRAIN_CORPUS_RELATION,
            incremental.BATCH_RELATION,
            incremental.PRIOR_CORPUS_RELATION,
        ):
            columns = connection.execute(f"DESCRIBE {relation}").fetchall()
            assert "metadata" not in {column[0] for column in columns}
        assert connection.execute("SELECT metadata FROM lake.main.int_std_records").fetchone() == (
            '{"client_only":"keep out of Splink"}',
        )


@pytest.fixture
def metadata_lake():
    with duckdb.connect() as connection:
        connection.execute("ATTACH ':memory:' AS lake")
        connection.execute("CREATE TABLE lake.main.entities(entity_id VARCHAR, status VARCHAR)")
        connection.execute(
            "INSERT INTO lake.main.entities VALUES ('e', 'active'), ('empty', 'active')"
        )
        connection.execute(
            "CREATE TABLE lake.main.entity_events "
            "(entity_id VARCHAR, run_id VARCHAR, event_type VARCHAR)"
        )
        connection.execute(
            "CREATE TABLE lake.main.entity_membership(entity_id VARCHAR, record_key VARCHAR)"
        )
        connection.execute(
            "INSERT INTO lake.main.entity_membership VALUES ('e', 'crm:a/~1'), ('empty', 'crm:b')"
        )
        connection.execute(
            "CREATE TABLE lake.main.int_std_records(record_key VARCHAR, source_system VARCHAR, "
            "source_record_id VARCHAR, metadata JSON)"
        )
        connection.execute(
            "INSERT INTO lake.main.int_std_records VALUES "
            "('crm:a/~1', 'crm', 'a/~1', '{\"tier\":\"gold\"}'), ('crm:b', 'crm', 'b', '{}')"
        )
        connection.execute(
            "CREATE TABLE lake.main.golden_records(entity_id VARCHAR, metadata JSON)"
        )
        connection.execute(
            "INSERT INTO lake.main.golden_records VALUES ('e', ?), ('empty', '{}')",
            [json.dumps({"crm": {"a/~1": {"tier": "silver"}}})],
        )
        connection.execute(
            "CREATE TABLE lake.main.er_touched_entities "
            "(run_id VARCHAR, entity_id VARCHAR, disposition VARCHAR, created_at TIMESTAMP)"
        )
        yield connection


def test_metadata_refresh_is_independent_of_events_and_idempotent(metadata_lake):
    connection = metadata_lake
    assert compute_touched_set(connection, "first") == {"e": "rebuild"}
    assert materialize_touched_entities(connection, "first", touched_only=True) == (1, 1)
    # A new run still sees metadata that has not reached golden_records.
    assert compute_touched_set(connection, "retry") == {"e": "rebuild"}
    connection.execute(
        "UPDATE lake.main.golden_records SET metadata = ? WHERE entity_id = 'e'",
        [json.dumps({"crm": {"a/~1": {"tier": "gold"}}}, separators=(",", ":"))],
    )
    assert compute_touched_set(connection, "done") == {}
    assert materialize_touched_entities(connection, "done", touched_only=True) == (0, 0)
    connection.execute(
        "UPDATE lake.main.int_std_records SET metadata = '{}' WHERE record_key='crm:a/~1'"
    )
    assert compute_touched_set(connection, "remove") == {"e": "rebuild"}


@pytest.mark.parametrize("failure_mode", ["raise", "return"])
def test_metadata_only_assembly_retries_after_golden_records_commits(
    metadata_lake, tmp_path, failure_mode
):
    connection = metadata_lake
    cfg = load_config(Path("configs/test.yaml"))
    run_id = "metadata-update"
    started_at = datetime(2026, 9, 25, 12)
    connection.execute("CREATE TABLE lake.main.runs(run_id VARCHAR, started_at TIMESTAMP)")
    connection.execute("INSERT INTO lake.main.runs VALUES (?, ?)", [run_id, started_at])
    connection.execute("ALTER TABLE lake.main.golden_records ADD assembled_at TIMESTAMP")
    connection.execute(
        "CREATE TABLE lake.main.golden_display AS "
        "SELECT entity_id, assembled_at FROM lake.main.golden_records"
    )
    connection.execute("CREATE TABLE lake.main.golden_lineage(entity_id VARCHAR, rule VARCHAR)")
    calls = []

    def runner(command, **kwargs):
        assert command == "build" and kwargs["select"] == "marts"
        calls.append(kwargs["vars"]["run_id"])
        connection.execute(
            "UPDATE lake.main.golden_records SET metadata = ?, assembled_at = ? "
            "WHERE entity_id = 'e'",
            [json.dumps({"crm": {"a/~1": {"tier": "gold"}}}, separators=(",", ":")), started_at],
        )
        exit_code = 0
        if len(calls) == 1:
            if failure_mode == "raise":
                raise StageFailure("golden_display failed after golden_records committed")
            exit_code = 1
        else:
            connection.execute(
                "UPDATE lake.main.golden_display SET assembled_at = ? WHERE entity_id = 'e'",
                [started_at],
            )
        return DbtResult(exit_code, (), None, None, tmp_path / "dbt.log")

    def attempt(attempt_run_id):
        return assemble(
            held(connection),
            cfg,
            run_id=attempt_run_id,
            counters=StageCounters(DECLARED_COUNTERS["assemble"]),
            touched_only=True,
            artifacts_dir=tmp_path,
            dbt=runner,
        )

    with pytest.raises(StageFailure):
        attempt(run_id)
    assert connection.execute("SELECT count(*) FROM lake.main.entity_events").fetchone() == (0,)
    assert connection.execute(
        "SELECT g.assembled_at, d.assembled_at FROM lake.main.golden_records g "
        "JOIN lake.main.golden_display d USING(entity_id) WHERE entity_id='e'"
    ).fetchone() == (started_at, None)

    # Matching metadata is insufficient: the saved work must still run every mart.
    assert compute_touched_set(connection, run_id) == {"e": "rebuild"}
    outcome = attempt(run_id)
    assert outcome.exit_code == 0
    assert outcome.entities_touched == outcome.entities_rebuilt == 1
    assert calls == [run_id, run_id]
    assert connection.execute(
        "SELECT assembled_at FROM lake.main.golden_display WHERE entity_id='e'"
    ).fetchone() == (started_at,)
    assert connection.execute(
        "SELECT entity_id, disposition FROM lake.main.er_touched_entities WHERE run_id=?",
        [run_id],
    ).fetchall() == [("e", "rebuild")]

    # A separate run has no outstanding work and must not repeat the completed marts.
    connection.execute("INSERT INTO lake.main.runs VALUES ('next', ?)", [started_at])
    assert attempt("next").exit_code == 10
    assert calls == [run_id, run_id]


def test_retry_unions_saved_and_new_work_with_current_dispositions(metadata_lake):
    connection = metadata_lake
    run_id = "retry"
    assert materialize_touched_entities(connection, run_id, touched_only=True) == (1, 1)
    connection.execute("UPDATE lake.main.entities SET status='retired' WHERE entity_id='e'")
    connection.execute("DELETE FROM lake.main.entity_membership WHERE entity_id='e'")
    connection.execute(
        'UPDATE lake.main.int_std_records SET metadata = \'{"tier":"silver"}\' '
        "WHERE record_key='crm:b'"
    )
    connection.execute(
        "INSERT INTO lake.main.entity_events VALUES ('empty', ?, 'member_added')", [run_id]
    )
    for _ in range(2):
        assert compute_touched_set(connection, run_id) == {"e": "retire", "empty": "rebuild"}
        assert materialize_touched_entities(connection, run_id, touched_only=True) == (2, 1)
        assert connection.execute(
            "SELECT entity_id, disposition FROM lake.main.er_touched_entities "
            "WHERE run_id=? ORDER BY entity_id",
            [run_id],
        ).fetchall() == [("e", "retire"), ("empty", "rebuild")]


def test_failed_touched_set_replacement_keeps_saved_work(metadata_lake):
    connection = metadata_lake
    # A local constraint injects a write failure after preparation's DELETE.
    connection.execute("DROP TABLE lake.main.er_touched_entities")
    connection.execute(
        "CREATE TABLE lake.main.er_touched_entities (run_id VARCHAR, entity_id VARCHAR, "
        "disposition VARCHAR CHECK (disposition = 'rebuild'), created_at TIMESTAMP)"
    )
    assert materialize_touched_entities(connection, "retry", touched_only=True) == (1, 1)
    saved = connection.execute("SELECT * FROM lake.main.er_touched_entities").fetchall()
    connection.execute("UPDATE lake.main.entities SET status='retired' WHERE entity_id='e'")
    with pytest.raises(duckdb.ConstraintException):
        materialize_touched_entities(connection, "retry", touched_only=True)
    assert connection.execute("SELECT * FROM lake.main.er_touched_entities").fetchall() == saved
    connection.execute("UPDATE lake.main.entities SET status='active' WHERE entity_id='e'")
    assert materialize_touched_entities(connection, "retry", touched_only=True) == (1, 1)
