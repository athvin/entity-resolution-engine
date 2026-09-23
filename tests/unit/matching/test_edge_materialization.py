"""A captured edge table is local, strict and explicitly refreshed by its owner."""

import duckdb
import pytest

from er.matching.edges import DuplicateEdgeKeyError, materialize_current_edges


def test_captured_edges_reuse_rows_without_changing_live_view_api() -> None:
    with duckdb.connect() as c:
        c.execute("ATTACH ':memory:' AS lake")
        c.execute(
            "CREATE TABLE lake.main.match_scores AS SELECT 'a' AS rec_a_key, 'b' AS rec_b_key, "
            "0.95::DOUBLE AS match_probability, 'v1' AS model_version, 'tf1' AS tf_snapshot_id, "
            "true AS is_active, TIMESTAMP '2026-01-01' AS scored_at, 'run1' AS run_id"
        )
        materialize_current_edges(c, "v1", "tf1", name="live_edges")
        materialize_current_edges(c, "v1", "tf1", name="saved_edges", materialized=True)
        assert c.execute(
            "SELECT temporary FROM duckdb_tables() WHERE table_name='saved_edges'"
        ).fetchone() == (True,)
        c.execute("UPDATE lake.main.match_scores SET match_probability=0.8")
        assert c.execute("SELECT match_probability FROM live_edges").fetchone() == (0.8,)
        assert c.execute("SELECT match_probability FROM saved_edges").fetchone() == (0.95,)
        materialize_current_edges(c, "v1", "tf1", name="saved_edges", materialized=True)
        assert c.execute("SELECT * FROM saved_edges").fetchall() == [("a", "b", 0.8)]
        c.execute("INSERT INTO lake.main.match_scores SELECT * FROM lake.main.match_scores")
        with pytest.raises(DuplicateEdgeKeyError):
            materialize_current_edges(c, "v1", "tf1", name="saved_edges", materialized=True)
