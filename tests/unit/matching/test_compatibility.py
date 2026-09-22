"""Migration refuses legacy models and requires completion through golden assembly."""

from datetime import datetime

import duckdb
import pytest

from er.errors import PreconditionFailure
from er.lake.model_registry import ModelRow
from er.matching.compatibility import assert_matching_model
from er.versions import PINS


def model(**metrics):
    return ModelRow(
        model_version="v0002",
        status="active",
        trained_at=datetime(2026, 1, 1),
        corpus_snapshot=1,
        params_path="unused",
        tf_tables_path="unused",
        tf_snapshot_id="tf",
        config_hash="config",
        metrics=metrics,
        run_id="train",
    )


@pytest.fixture
def connection():
    with duckdb.connect() as con:
        con.execute("ATTACH ':memory:' AS lake")
        con.execute("CREATE TABLE lake.main.runs (run_id VARCHAR, model_version VARCHAR)")
        con.execute(
            "CREATE TABLE lake.main.run_stages (run_id VARCHAR, stage VARCHAR, status VARCHAR, "
            "started_at TIMESTAMP, ended_at TIMESTAMP, counters JSON)"
        )
        con.execute("INSERT INTO lake.main.runs VALUES ('resolution', 'v0002')")
        yield con


@pytest.mark.parametrize("version", [None, "4.0.16", "5.0.0.dev4"])
@pytest.mark.parametrize("incremental", [False, True])
def test_legacy_and_different_prerelease_models_are_refused(connection, version, incremental):
    with pytest.raises(PreconditionFailure, match="Train a new model"):
        assert_matching_model(connection, model(splink_version=version), incremental=incremental)
    assert connection.execute("SELECT count(*) FROM lake.main.run_stages").fetchone() == (0,)


def test_migration_requires_full_match_reconcile_and_assemble(connection):
    migrated = model(splink_version=PINS["splink"].version, migration_requires_full_resolution=True)
    assert_matching_model(connection, migrated, incremental=False)
    for stage, hour in (("match", 1), ("reconcile", 2), ("assemble", 3)):
        with pytest.raises(PreconditionFailure, match="successful full matching"):
            assert_matching_model(connection, migrated, incremental=True)
        connection.execute(
            "INSERT INTO lake.main.run_stages VALUES ('resolution', ?, 'succeeded', ?, ?, ?)",
            [stage, datetime(2026, 1, 1, hour), datetime(2026, 1, 1, hour, 1), '{"mode":"full"}'],
        )
    assert_matching_model(connection, migrated, incremental=True)
    connection.execute("UPDATE lake.main.run_stages SET status='failed' WHERE stage='assemble'")
    with pytest.raises(PreconditionFailure):
        assert_matching_model(connection, migrated, incremental=True)


def test_fresh_v5_model_does_not_require_a_migration(connection):
    assert_matching_model(
        connection, model(splink_version=PINS["splink"].version), incremental=True
    )


def test_if_changed_never_reuses_a_legacy_model():
    from er.matching.train import _unchanged

    assert not _unchanged(model(), "config", 1)
    assert not _unchanged(model(splink_version="4.0.16"), "config", 1)
    assert _unchanged(model(splink_version=PINS["splink"].version), "config", 1)
