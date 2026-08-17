"""The current edge set against a real DuckLake namespace (S4.5.1, S4.3.4, S4.0b, S8.1).

Three of this module's four claims are only true of a real lake and are unassertable
against an ATTACHed in-memory stand-in, which is why they live here rather than beside
the SQL-shape tests in `tests/unit/matching/test_current_edges.py`:

* **Invalidation removes an edge and deletes nothing.** S4.3.4 makes invalidation an
  in-place `UPDATE` and `match_scores` cumulative — "never truncated per run". The half
  that matters operationally is the second one: the row survives, so a superseded pair
  vanishes from the edge set with its evidence intact (S4.5.5). Asserting the count is
  unchanged is what distinguishes "excluded" from "deleted", and a relation living in
  DuckLake is what makes the claim about the storage layer the pipeline actually uses.
* **Nothing is created in `lake`.** S4.0b confines the S4.5 loop's intermediates to the
  in-memory database precisely so the loop cannot commit one DuckLake snapshot per
  iteration. The only honest way to assert that is to enumerate `lake.main` before and
  after against a catalog that would really have gained a relation.
* **The key filter.** `match_scores` mixes rows written by different runs under
  different `(model_version, tf_snapshot_id)` keys (S4.3.3, INV-SCORE), and the keys
  used here are the committed fixture model's own, obtained the way every scenario
  obtains them (S4.3.2 item 6).

The rows are inserted directly rather than scored. `er match --mode full` is
`tests/integration/test_full_match.py`'s subject; what is under test here is the
*reader*, and a scoring run would make the assertions depend on which pairs `base_10`
happens to put above a threshold. Direct inserts also let one row carry the
`is_active=false` state S4.5.5 produces, which this milestone has no writer for yet.
"""

from __future__ import annotations

from collections.abc import Iterator
from datetime import datetime
from typing import Any, Final

import duckdb
import pytest
from helpers.model import load_fixture_model

from er.lake.model import REGISTRY, SCHEMA_QUALIFIER
from er.matching.edges import (
    CURRENT_EDGES_RELATION,
    MATCH_SCORES_RELATION,
    current_edges,
    materialize_current_edges,
)

MATCH_SCORES: Final = f"{SCHEMA_QUALIFIER}.{MATCH_SCORES_RELATION}"

#: S5's `match_scores` column list, in DDL order, read from the one place that declares
#: it — the same source `score_full`'s merge is generated from.
MATCH_SCORE_COLUMNS: Final[tuple[str, ...]] = REGISTRY[MATCH_SCORES_RELATION].column_names

_LAKE_DATABASE, _LAKE_SCHEMA = SCHEMA_QUALIFIER.split(".", 1)

#: Four canonical pairs (S5.0), spelled in `source_system:source_record_id` form so they
#: are record keys rather than opaque strings.
PAIR_AB: Final[tuple[str, str]] = ("crm:a", "crm:b")
PAIR_CD: Final[tuple[str, str]] = ("crm:c", "crm:d")
PAIR_EF: Final[tuple[str, str]] = ("billing:e", "billing:f")

#: A second key, standing in for a re-score under a different model and TF snapshot. Its
#: rows must never appear in a read at the fixture model's key (INV-SCORE, S4.3.3).
OTHER_MODEL_VERSION: Final = "v9999"
OTHER_TF_SNAPSHOT_ID: Final = "01JWMDTFSNAP00000000009999"

RUN_ID: Final = "01JWMDRN0000000000000000ED"
SCORED_AT: Final = datetime(2026, 2, 1, 0, 0, 0)


@pytest.fixture
def scored_lake(initialised_lake: duckdb.DuckDBPyConnection) -> Iterator[tuple[str, str]]:
    """An initialised namespace holding the fixture model and a small edge set.

    The keys come from :func:`~helpers.model.load_fixture_model` rather than from two
    literals, because they are the keys a scenario really reads under: a helper that
    invented its own would assert nothing about the `(model_version, tf_snapshot_id)`
    pair the rest of the suite scores at.
    """
    model_version, tf_snapshot_id, _ = load_fixture_model(initialised_lake)

    insert_edge(initialised_lake, PAIR_AB, 0.99, model_version, tf_snapshot_id)
    insert_edge(initialised_lake, PAIR_CD, 0.80, model_version, tf_snapshot_id)
    # The same pair under a different key: a re-score, not a duplicate logical key.
    insert_edge(initialised_lake, PAIR_AB, 0.10, OTHER_MODEL_VERSION, OTHER_TF_SNAPSHOT_ID)
    insert_edge(initialised_lake, PAIR_EF, 0.75, OTHER_MODEL_VERSION, OTHER_TF_SNAPSHOT_ID)

    try:
        yield model_version, tf_snapshot_id
    finally:
        # The materialized form is a TEMP view on the session connection, which S8.1's
        # function isolation does not reach: it empties the lake's relations, and a temp
        # view is deliberately not one of them. Dropping it here keeps the leftover from
        # outliving the `match_scores` rows it selects from.
        initialised_lake.execute(f"DROP VIEW IF EXISTS {CURRENT_EDGES_RELATION}")


def insert_edge(
    connection: duckdb.DuckDBPyConnection,
    pair: tuple[str, str],
    match_probability: float,
    model_version: str,
    tf_snapshot_id: str,
) -> None:
    """One active `match_scores` row, positionally in S5's column order."""
    rec_a_key, rec_b_key = pair
    row: tuple[Any, ...] = (
        rec_a_key,
        rec_b_key,
        match_probability,
        model_version,
        tf_snapshot_id,
        f"{rec_a_key}-hash",
        f"{rec_b_key}-hash",
        "{}",
        True,
        None,
        None,
        RUN_ID,
        SCORED_AT,
    )
    assert len(row) == len(MATCH_SCORE_COLUMNS), (
        f"{MATCH_SCORES_RELATION} declares {len(MATCH_SCORE_COLUMNS)} columns and this "
        f"helper supplies {len(row)}; the values are positional (S5)"
    )
    connection.execute(
        f"INSERT INTO {MATCH_SCORES} ({', '.join(MATCH_SCORE_COLUMNS)}) "
        f"VALUES ({', '.join('?' for _ in MATCH_SCORE_COLUMNS)})",
        list(row),
    )


def scalar(connection: duckdb.DuckDBPyConnection, statement: str, *parameters: Any) -> Any:
    row = connection.execute(statement, list(parameters)).fetchone()
    assert row is not None, f"{statement!r} returned no row"
    return row[0]


def relations_in_lake(connection: duckdb.DuckDBPyConnection) -> frozenset[str]:
    """Every relation live in `lake.main` right now, tables and views alike.

    Both, because AC6's claim is that *nothing* was created there and a temporary view
    created in the lake's schema would be invisible to a check that enumerated only
    tables — which is exactly the mistake the claim exists to catch.
    """
    tables = connection.execute(
        "SELECT table_name FROM duckdb_tables() WHERE database_name = ? AND schema_name = ?",
        [_LAKE_DATABASE, _LAKE_SCHEMA],
    ).fetchall()
    views = connection.execute(
        "SELECT view_name FROM duckdb_views() WHERE database_name = ? AND schema_name = ?",
        [_LAKE_DATABASE, _LAKE_SCHEMA],
    ).fetchall()
    return frozenset(str(name) for (name,) in [*tables, *views])


def assert_canonical_edge_set(edges: list[tuple[str, str, float]]) -> None:
    """AC4, asserted on every result this module produces.

    Canonical ordering is a write-time invariant (S5.0) and pair uniqueness is what
    "one row per canonical pair" means, so neither is interesting on its own result —
    they are interesting on *every* result, which is why this is a helper rather than a
    test. A reader that returned `(b, a)` would make every downstream one-sided join
    silently miss the edge.
    """
    for rec_a_key, rec_b_key, _ in edges:
        assert rec_a_key < rec_b_key, (
            f"({rec_a_key!r}, {rec_b_key!r}) is not canonical; S5.0 requires "
            f"rec_a_key < rec_b_key on every pair row"
        )
    pairs = [(rec_a_key, rec_b_key) for rec_a_key, rec_b_key, _ in edges]
    assert len(pairs) == len(set(pairs)), f"the result repeats a pair: {pairs}"


def test_filters_by_model_version_and_tf_snapshot(
    scored_lake: tuple[str, str], initialised_lake: duckdb.DuckDBPyConnection
) -> None:
    """AC1, AC4: exactly the rows at the requested key, canonical and pair-unique.

    `match_scores` is cumulative and mixes keys (S4.3.3, S4.3.4), so the pair scored
    under both keys must come back once, carrying the probability of the key that was
    asked for — and the pair scored only under the other key must not appear at all.
    """
    model_version, tf_snapshot_id = scored_lake

    edges = current_edges(initialised_lake, model_version, tf_snapshot_id)

    assert_canonical_edge_set(edges)
    assert edges == [(*PAIR_AB, 0.99), (*PAIR_CD, 0.80)]

    other = current_edges(initialised_lake, OTHER_MODEL_VERSION, OTHER_TF_SNAPSHOT_ID)
    assert_canonical_edge_set(other)
    assert other == [(*PAIR_EF, 0.75), (*PAIR_AB, 0.10)]

    # The inclusive bound of S4.5.1, against the real relation: `PAIR_CD` sits exactly
    # on it and stays, and `PAIR_AB` is above it.
    bounded = current_edges(initialised_lake, model_version, tf_snapshot_id, min_probability=0.80)
    assert_canonical_edge_set(bounded)
    assert bounded == edges


def test_excludes_invalidated_edges_without_deleting_rows(
    scored_lake: tuple[str, str], initialised_lake: duckdb.DuckDBPyConnection
) -> None:
    """AC2: `is_active=false` removes the edge; the row is still there (S4.3.4, S4.5.5).

    The two halves are one claim. An implementation that deleted the row would pass the
    exclusion assertion and destroy the evidence a re-score needs, and one that filtered
    on `run_id` instead of on `is_active` would pass neither.
    """
    model_version, tf_snapshot_id = scored_lake
    rec_a_key, rec_b_key = PAIR_CD

    before = scalar(initialised_lake, f"SELECT count(*) FROM {MATCH_SCORES}")
    assert current_edges(initialised_lake, model_version, tf_snapshot_id) == [
        (*PAIR_AB, 0.99),
        (*PAIR_CD, 0.80),
    ]

    # S4.5.5's invalidation, in place: exactly the three columns S4.3.4 names.
    initialised_lake.execute(
        f"UPDATE {MATCH_SCORES} SET is_active = false, invalidated_at = ?, "
        f"invalidated_run_id = ? WHERE rec_a_key = ? AND rec_b_key = ? "
        f"AND model_version = ? AND tf_snapshot_id = ?",
        [SCORED_AT, RUN_ID, rec_a_key, rec_b_key, model_version, tf_snapshot_id],
    )

    active = current_edges(initialised_lake, model_version, tf_snapshot_id)
    assert_canonical_edge_set(active)
    assert active == [(*PAIR_AB, 0.99)], "an invalidated edge is not in the edge set"

    assert scalar(initialised_lake, f"SELECT count(*) FROM {MATCH_SCORES}") == before, (
        "invalidation is an in-place UPDATE; match_scores is cumulative and is never "
        "truncated per run (S4.3.4)"
    )
    assert (
        scalar(
            initialised_lake,
            f"SELECT count(*) FROM {MATCH_SCORES} WHERE NOT is_active AND "
            f"invalidated_run_id IS NOT NULL",
        )
        == 1
    )

    restored = current_edges(initialised_lake, model_version, tf_snapshot_id, include_inactive=True)
    assert_canonical_edge_set(restored)
    assert restored == [(*PAIR_AB, 0.99), (*PAIR_CD, 0.80)]


def test_materializes_outside_the_lake(
    scored_lake: tuple[str, str], initialised_lake: duckdb.DuckDBPyConnection
) -> None:
    """AC6: `lake.main` is untouched, and the returned name composes (S4.0b).

    S4.0b confines the S4.5 loop's iterations to the in-memory database and writes only
    the final labelling to the lake. A helper that materialized into `lake` would commit
    a DuckLake snapshot for what is a working value, so "the relation set is identical
    before and after" is the claim, not "no table with this name appeared".
    """
    model_version, tf_snapshot_id = scored_lake
    before = relations_in_lake(initialised_lake)

    name = materialize_current_edges(
        initialised_lake, model_version, tf_snapshot_id, min_probability=0.90
    )

    assert name == CURRENT_EDGES_RELATION
    assert relations_in_lake(initialised_lake) == before, (
        f"materialize_current_edges created a relation in {SCHEMA_QUALIFIER}: "
        f"{sorted(relations_in_lake(initialised_lake) - before)}"
    )

    # Queryable as a subquery inside a composed statement, which is the whole point of
    # returning a name rather than a result set.
    composed = initialised_lake.execute(
        f"SELECT rec_a_key, rec_b_key, match_probability FROM "
        f"(SELECT * FROM {name}) AS edges "
        f"WHERE rec_a_key IN (SELECT rec_a_key FROM {name}) "
        f"ORDER BY rec_a_key, rec_b_key"
    ).fetchall()
    assert composed == [(*PAIR_AB, 0.99)]

    # Re-materializing under a different bound replaces the relation rather than
    # failing, and still creates nothing in the lake.
    materialize_current_edges(initialised_lake, model_version, tf_snapshot_id)
    assert relations_in_lake(initialised_lake) == before
    assert sorted(initialised_lake.execute(f"SELECT * FROM {name}").fetchall()) == [
        (*PAIR_AB, 0.99),
        (*PAIR_CD, 0.80),
    ]


def test_result_is_stable_across_calls(
    scored_lake: tuple[str, str], initialised_lake: duckdb.DuckDBPyConnection
) -> None:
    """AC7: the same arguments over an unchanged relation give the same rows.

    Order-insensitively, as the criterion states — the reader is a set-returning query
    and nothing downstream may depend on a row order the engine never promised. The
    eager form does sort its result, so the stronger sequence equality is asserted too:
    a helper that is deterministic is one fewer thing a clustering run's determinism
    (INV-PERM, S4.5.4) has to be argued around.
    """
    model_version, tf_snapshot_id = scored_lake

    first = current_edges(initialised_lake, model_version, tf_snapshot_id)
    second = current_edges(initialised_lake, model_version, tf_snapshot_id)

    assert sorted(first) == sorted(second)
    assert first == second
    assert first, "a stability claim over an empty result asserts nothing"

    # And through the materialized form, which is the same query behind a name.
    name = materialize_current_edges(initialised_lake, model_version, tf_snapshot_id)
    rows = [initialised_lake.execute(f"SELECT * FROM {name}").fetchall() for _ in range(2)]
    assert sorted(rows[0]) == sorted(rows[1]) == sorted(first)
