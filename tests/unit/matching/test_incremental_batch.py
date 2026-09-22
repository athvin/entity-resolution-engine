"""Batch discovery retains content/version semantics without a quadratic OR join."""

import duckdb

from er.matching.incremental import _UNSCORED_KEYS_SQL, unscored_record_keys


def test_batch_discovery_preserves_historical_score_and_batch_semantics() -> None:
    with duckdb.connect() as connection:
        connection.execute("ATTACH ':memory:' AS lake")
        connection.execute(
            "CREATE TABLE lake.main.int_std_records AS SELECT * FROM (VALUES "
            "('crm:1', 'h1', 'first'), ('crm:2', 'h2', 'first'), ('crm:3', 'h3', 'first'), "
            "('crm:4', 'h4', 'changed'), ('crm:5', 'h5', 'new'), ('crm:6', 'h6', 'new'), "
            "('crm:7', 'h7', 'inactive'), ('crm:8', 'h8', 'unmatched')) "
            "t(record_key, content_hash, ingest_batch_id)"
        )
        connection.execute(
            "CREATE TABLE lake.main.match_scores AS SELECT * FROM (VALUES "
            "('crm:1', 'crm:2', 'h1', 'h2', 'v1', 'tf1', true), "
            "('crm:1', 'crm:4', 'h1', 'old-h4', 'v1', 'tf1', false), "
            "('crm:5', 'crm:6', 'h5', 'h6', 'v0', 'tf1', true), "
            "('crm:5', 'crm:6', 'h5', 'h6', 'v1', 'tf0', true), "
            "('crm:1', 'crm:7', 'h1', 'h7', 'v1', 'tf1', false)) "
            "t(rec_a_key, rec_b_key, rec_a_content_hash, rec_b_content_hash, "
            "model_version, tf_snapshot_id, is_active)"
        )
        # Unmatched records in a previously scored batch remain accounted for;
        # old endpoint hashes, different models and different TF snapshots do not.
        assert unscored_record_keys(connection, model_version="v1", tf_snapshot_id="tf1") == (
            "crm:4",
            "crm:5",
            "crm:6",
            "crm:8",
        )


def test_large_batch_discovery_avoids_pairwise_nested_loop() -> None:
    with duckdb.connect(config={"threads": 2}) as connection:
        connection.execute("ATTACH ':memory:' AS lake")
        connection.execute(
            "CREATE TABLE lake.main.int_std_records AS SELECT 'crm:' || i record_key, "
            "'h' || i content_hash, i::VARCHAR ingest_batch_id FROM range(100000) t(i)"
        )
        connection.execute(
            "CREATE TABLE lake.main.match_scores AS SELECT 'crm:' || i rec_a_key, "
            "'crm:' || (i + 1) rec_b_key, 'h' || i rec_a_content_hash, "
            "'h' || (i + 1) rec_b_content_hash, 'v1' model_version, 'tf1' tf_snapshot_id "
            "FROM range(90000) t(i)"
        )
        plan = connection.execute("EXPLAIN " + _UNSCORED_KEYS_SQL, ["v1", "tf1"]).fetchone()
        assert plan is not None
        assert "BLOCKWISE_NL_JOIN" not in plan[1]
        result = unscored_record_keys(connection, model_version="v1", tf_snapshot_id="tf1")
        assert len(result) == 9999
        assert all(int(key.removeprefix("crm:")) > 90000 for key in result)
