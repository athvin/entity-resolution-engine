"""Recovery uses pending batch identities even after staging has committed."""

from pathlib import Path

import duckdb
import pytest

from er.config.loader import load_config
from er.std.stage import WORK_RELATION, prepare_work


@pytest.fixture
def connection():
    with duckdb.connect() as con:
        con.execute("ATTACH ':memory:' AS lake")
        con.execute(
            "CREATE TABLE lake.main.raw_records (source_system VARCHAR, "
            "source_record_id VARCHAR, ingest_batch_id VARCHAR)"
        )
        con.execute("CREATE TABLE lake.main.stg_crm (ingest_batch_id VARCHAR)")
        con.execute("CREATE TABLE lake.main.int_std_records (record_key VARCHAR)")
        con.execute("CREATE TABLE lake.main.int_blocking_keys (record_key VARCHAR)")
        con.execute(
            "INSERT INTO lake.main.raw_records VALUES "
            "('crm', 'a', 'old'), ('crm', 'a', 'new'), ('crm', 'b', 'new')"
        )
        con.execute("INSERT INTO lake.main.stg_crm VALUES ('old')")
        yield con


def config():
    return load_config(Path(__file__).resolve().parents[2] / "configs/test.yaml")


def test_partial_staging_is_recovered_in_same_and_new_run(connection):
    assert prepare_work(connection, config(), "first", full_refresh=False) == (2, True, False)
    # Staging succeeded; current records/blocking failed. Its journal must survive.
    connection.execute("INSERT INTO lake.main.stg_crm VALUES ('new')")
    assert prepare_work(connection, config(), "first", full_refresh=False) == (2, True, False)
    assert prepare_work(connection, config(), "retry", full_refresh=False) == (2, True, False)
    assert connection.execute(
        f"SELECT run_id, ingest_batch_id FROM {WORK_RELATION} ORDER BY run_id"
    ).fetchall() == [("first", "new"), ("retry", "new")]
    connection.execute(f"DELETE FROM {WORK_RELATION}")
    assert prepare_work(connection, config(), "done", full_refresh=False) == (0, True, False)


def test_full_refresh_survives_retry_and_selects_history(connection):
    assert prepare_work(connection, config(), "first", full_refresh=True) == (3, False, True)
    connection.execute("INSERT INTO lake.main.stg_crm VALUES ('new')")
    assert prepare_work(connection, config(), "retry", full_refresh=False) == (3, False, True)


def test_missing_derived_relation_cannot_skip_staged_batches(connection):
    connection.execute("DROP TABLE lake.main.int_blocking_keys")
    connection.execute("INSERT INTO lake.main.stg_crm VALUES ('new')")
    assert prepare_work(connection, config(), "repair", full_refresh=False) == (3, False, False)


def test_full_scope_rebuilds_keys_even_without_a_new_delivery(connection):
    connection.execute("INSERT INTO lake.main.stg_crm VALUES ('new')")
    assert prepare_work(
        connection, config(), "rules_changed", full_refresh=False, changed_only=False
    ) == (3, False, False)
