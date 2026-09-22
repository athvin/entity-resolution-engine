"""A full correction must repair marts left by an earlier interrupted run."""

import duckdb

from er.golden.assemble import materialize_touched_entities, reap_retired_entities


def test_full_assembly_reaps_historical_retirements_without_current_events():
    with duckdb.connect() as connection:
        connection.execute("ATTACH ':memory:' AS lake")
        connection.execute("CREATE TABLE lake.main.entities (entity_id VARCHAR, status VARCHAR)")
        connection.execute(
            "INSERT INTO lake.main.entities VALUES "
            "('survivor', 'active'), ('loser', 'merged'), ('deleted', 'retired')"
        )
        connection.execute(
            "CREATE TABLE lake.main.er_touched_entities "
            "(run_id VARCHAR, entity_id VARCHAR, disposition VARCHAR, created_at TIMESTAMP)"
        )
        for table in ("golden_records", "golden_lineage", "golden_display"):
            connection.execute(f"CREATE TABLE lake.main.{table} (entity_id VARCHAR)")
            connection.execute(
                f"INSERT INTO lake.main.{table} VALUES ('survivor'), ('loser'), ('deleted')"
            )

        # There are no current-run events. Full repair uses current entity status,
        # including both forms of inactive entity left by previous runs.
        for run_id in ("repair", "retry"):
            materialize_touched_entities(connection, run_id, touched_only=False)
            assert reap_retired_entities(connection, run_id) == 2
            for table in ("golden_records", "golden_lineage", "golden_display"):
                assert connection.execute(
                    f"SELECT entity_id FROM lake.main.{table}"
                ).fetchall() == [("survivor",)]
