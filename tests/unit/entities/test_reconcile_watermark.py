"""A pipeline run starts before ingestion; reconciliation needs its own cutoff."""

from datetime import datetime

import duckdb

from er.entities.cluster import last_reconciled_watermark


def test_watermark_uses_the_latest_successful_stage_start() -> None:
    with duckdb.connect() as connection:
        connection.execute("ATTACH ':memory:' AS lake")
        connection.execute("CREATE TABLE lake.main.runs (run_id VARCHAR, started_at TIMESTAMP)")
        connection.execute(
            "CREATE TABLE lake.main.run_stages "
            "(run_id VARCHAR, stage VARCHAR, status VARCHAR, started_at TIMESTAMP)"
        )
        assert last_reconciled_watermark(connection) is None
        connection.execute(
            "INSERT INTO lake.main.runs VALUES "
            "('early', '2026-01-01 01:00:00'), ('late', '2026-01-01 02:00:00'), "
            "('failed', '2026-01-01 04:00:00')"
        )
        connection.execute(
            "INSERT INTO lake.main.run_stages VALUES "
            "('early', 'reconcile', 'succeeded', '2026-01-01 03:00:00'), "
            "('late', 'reconcile', 'succeeded', '2026-01-01 02:30:00'), "
            "('failed', 'reconcile', 'failed', '2026-01-01 05:00:00'), "
            "('early', 'assemble', 'succeeded', '2026-01-01 06:00:00')"
        )
        assert last_reconciled_watermark(connection) == datetime(2026, 1, 1, 3)
