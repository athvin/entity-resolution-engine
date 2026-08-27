"""S4.5.5 invalidation, over a hand-built lake (S4.3.3, S5.0).

Two properties, and the reason each needs its own case:

* **Nothing is touched when nothing moved.** The dangerous failure is not an
  invalidation that misses a stale edge — that shows up as a wrong cluster almost
  immediately — it is one that retires edges it should have left alone. Every run calls
  this function, so an over-broad predicate would quietly deactivate the whole corpus on
  the first re-score and every subsequent partition would be built from nothing.
* **It never leaves a second row.** `match_scores` permits at most one row per
  `(rec_a_key, rec_b_key, model_version, tf_snapshot_id)` regardless of `is_active`
  (S5.0), so invalidation has to be an `UPDATE`. An implementation that inserted a
  superseding row would pass every "is the edge inactive?" assertion and silently double
  every logical key.

The relations are built here rather than through `er init`, so the unit layer needs no
lake and no services (S8.1). Only the columns the predicate reads are populated; the
rest of S5's column list is irrelevant to what is under test and spelling it out would
make this a schema test.
"""

from __future__ import annotations

from collections.abc import Iterator
from datetime import UTC, datetime
from typing import Final

import duckdb
import pytest

from er.entities.retraction import invalidate_incident_edges, stale_edge_rows

#: The schema the module qualifies its reads with, created locally so the unqualified
#: `lake.main` names resolve on a bare in-memory connection.
LAKE: Final = "lake"
SCHEMA: Final = "main"

STAMP: Final = datetime(2026, 3, 1, tzinfo=UTC).replace(tzinfo=None)
RUN: Final = "01M0RUN00000000000000000RT"

MODEL: Final = "v0001"
SNAPSHOT: Final = "01JWMDTFSNAP00000000000001"


@pytest.fixture
def lake() -> Iterator[duckdb.DuckDBPyConnection]:
    """An in-memory stand-in for the two relations the predicate joins."""
    connection = duckdb.connect()
    connection.execute(f"ATTACH ':memory:' AS {LAKE}")
    connection.execute(f"CREATE SCHEMA IF NOT EXISTS {LAKE}.{SCHEMA}")
    connection.execute(
        f"""CREATE TABLE {LAKE}.{SCHEMA}.int_std_records (
                record_key VARCHAR, content_hash VARCHAR)"""
    )
    connection.execute(
        f"""CREATE TABLE {LAKE}.{SCHEMA}.match_scores (
                rec_a_key VARCHAR, rec_b_key VARCHAR, match_probability DOUBLE,
                model_version VARCHAR, tf_snapshot_id VARCHAR,
                rec_a_content_hash VARCHAR, rec_b_content_hash VARCHAR,
                is_active BOOLEAN, invalidated_at TIMESTAMP, invalidated_run_id VARCHAR)"""
    )
    try:
        yield connection
    finally:
        connection.close()


def add_record(connection: duckdb.DuckDBPyConnection, key: str, digest: str) -> None:
    connection.execute(f"INSERT INTO {LAKE}.{SCHEMA}.int_std_records VALUES (?, ?)", [key, digest])


def add_edge(
    connection: duckdb.DuckDBPyConnection,
    left: str,
    right: str,
    left_hash: str,
    right_hash: str,
) -> None:
    connection.execute(
        f"INSERT INTO {LAKE}.{SCHEMA}.match_scores VALUES (?, ?, ?, ?, ?, ?, ?, true, NULL, NULL)",
        [left, right, 0.99, MODEL, SNAPSHOT, left_hash, right_hash],
    )


def rows(connection: duckdb.DuckDBPyConnection) -> list[tuple[str, str, bool]]:
    return [
        (str(a), str(b), bool(active))
        for a, b, active in connection.execute(
            f"SELECT rec_a_key, rec_b_key, is_active FROM {LAKE}.{SCHEMA}.match_scores "
            "ORDER BY rec_a_key, rec_b_key"
        ).fetchall()
    ]


def test_invalidate_only_touches_stale_endpoint_hashes(
    lake: duckdb.DuckDBPyConnection,
) -> None:
    """AC4: a corpus that did not move retires nothing, and a moved endpoint retires its edges."""
    add_record(lake, "crm:C1", "h1")
    add_record(lake, "crm:C2", "h2")
    add_record(lake, "crm:C3", "h3")
    add_edge(lake, "crm:C1", "crm:C2", "h1", "h2")
    add_edge(lake, "crm:C2", "crm:C3", "h2", "h3")

    # Nothing has moved: the run must be a no-op. This is the arm that matters, because
    # every run calls this and an over-broad predicate would retire the whole corpus.
    assert invalidate_incident_edges(lake, run_id=RUN, now=STAMP) == 0
    assert stale_edge_rows(lake) == []
    assert rows(lake) == [("crm:C1", "crm:C2", True), ("crm:C2", "crm:C3", True)]

    # C2 is re-delivered with corrected values: both edges incident to it are now about
    # a version that no longer exists, and neither edge's own endpoints changed keys.
    lake.execute(
        f"UPDATE {LAKE}.{SCHEMA}.int_std_records SET content_hash = 'h2b' WHERE record_key = ?",
        ["crm:C2"],
    )
    stale = stale_edge_rows(lake)
    assert [(a, b) for a, b, _ in stale] == [("crm:C1", "crm:C2"), ("crm:C2", "crm:C3")]
    assert stale[0][2] == "rec_b hash changed"
    assert stale[1][2] == "rec_a hash changed"

    assert invalidate_incident_edges(lake, run_id=RUN, now=STAMP) == 2
    assert rows(lake) == [("crm:C1", "crm:C2", False), ("crm:C2", "crm:C3", False)]

    stamped = lake.execute(
        f"SELECT invalidated_at, invalidated_run_id FROM {LAKE}.{SCHEMA}.match_scores"
    ).fetchall()
    for invalidated_at, invalidated_run_id in stamped:
        assert invalidated_at is not None, "an invalidated row carries no stamp (S5)"
        assert str(invalidated_run_id) == RUN, "the retirement is not traceable to its run"

    # And it is idempotent: a second call finds nothing still active to retire.
    assert invalidate_incident_edges(lake, run_id=RUN, now=STAMP) == 0


def test_invalidate_never_inserts_a_second_row(lake: duckdb.DuckDBPyConnection) -> None:
    """AC3/AC4: the logical key holds one row before and after (S5.0).

    An implementation that recorded the retirement by inserting a superseding row would
    satisfy every "is the edge inactive?" assertion and double the key — which the
    S4.3.4 MERGE would then find ambiguous on the next re-score.
    """
    add_record(lake, "crm:C1", "h1")
    add_record(lake, "crm:C2", "h2")
    add_edge(lake, "crm:C1", "crm:C2", "h1", "h2")

    def keys() -> int:
        row = lake.execute(
            f"SELECT count(*) FROM {LAKE}.{SCHEMA}.match_scores "
            "GROUP BY rec_a_key, rec_b_key, model_version, tf_snapshot_id "
            "ORDER BY count(*) DESC LIMIT 1"
        ).fetchone()
        return 0 if row is None else int(row[0])

    assert keys() == 1
    lake.execute(
        f"UPDATE {LAKE}.{SCHEMA}.int_std_records SET content_hash = 'h1b' WHERE record_key = ?",
        ["crm:C1"],
    )
    assert invalidate_incident_edges(lake, run_id=RUN, now=STAMP) == 1
    assert keys() == 1, "invalidation added a second row for one logical key (S5.0)"
    assert (
        int(lake.execute(f"SELECT count(*) FROM {LAKE}.{SCHEMA}.match_scores").fetchone()[0]) == 1
    )


def test_absent_endpoint_is_stale(lake: duckdb.DuckDBPyConnection) -> None:
    """A tombstoned endpoint retires its edges (S4.2, S4.5.5).

    S4.2 excludes a tombstoned record from the standardized corpus entirely, so absence
    IS the tombstone. An equality-only predicate would find no differing hash and leave
    the edge clustering a record that no longer exists.
    """
    add_record(lake, "crm:C1", "h1")
    add_edge(lake, "crm:C1", "crm:C2", "h1", "h2")  # C2 never standardized

    stale = stale_edge_rows(lake)
    assert [(a, b, reason) for a, b, reason in stale] == [("crm:C1", "crm:C2", "rec_b absent")]
    assert invalidate_incident_edges(lake, run_id=RUN, now=STAMP) == 1
    assert rows(lake) == [("crm:C1", "crm:C2", False)]


def test_already_inactive_rows_are_left_alone(lake: duckdb.DuckDBPyConnection) -> None:
    """An inactive row is not re-invalidated, so its original stamp survives.

    `invalidated_run_id` names the run that retired the edge. Re-stamping it on every
    later run would make the column say "the most recent run", which is not a fact
    anybody needs and destroys the one it replaced.
    """
    add_record(lake, "crm:C1", "h1")
    add_record(lake, "crm:C2", "h2")
    lake.execute(
        f"INSERT INTO {LAKE}.{SCHEMA}.match_scores VALUES (?, ?, ?, ?, ?, ?, ?, false, ?, ?)",
        ["crm:C1", "crm:C2", 0.99, MODEL, SNAPSHOT, "stale", "stale", STAMP, "earlier-run"],
    )

    assert invalidate_incident_edges(lake, run_id=RUN, now=STAMP) == 0
    recorded = lake.execute(
        f"SELECT invalidated_run_id FROM {LAKE}.{SCHEMA}.match_scores"
    ).fetchone()
    assert recorded is not None and str(recorded[0]) == "earlier-run", (
        "an already-inactive row was re-stamped with a later run"
    )
