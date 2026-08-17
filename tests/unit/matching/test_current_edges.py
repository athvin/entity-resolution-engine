"""One row per canonical pair, and what happens when the logical key is not one (S4.5.1).

The four claims this layer can make without a lake are the four that are really about
the *query* rather than about DuckLake: that the key filter picks a row per pair, that a
violated logical key is refused rather than silently resolved, that `strict=False`
resolves it by the documented total order and not by whatever the engine returns first,
and that the probability bound is inclusive at exactly `auto_merge`.

Two of those need rows that S4.3.4's `MERGE INTO` can never write. A duplicate logical
key is the whole subject of :class:`~er.matching.edges.DuplicateEdgeKeyError`, and it
exists precisely because DuckLake enforces no `UNIQUE` constraint (S5.0) — so the only
way to test the refusal is to insert the row the writer would not. The rows here are
therefore inserted directly, which is also why this is the unit layer: nothing about
these assertions improves for being made against a real lake, and a duplicate key
deliberately planted in one would trip the T-INV-1 finalizer of every test after it.

`lake.main.match_scores` is an in-memory database ATTACHed under the lake's alias and
created from `REGISTRY`'s own `TableSpec`, so the relation under test is the one S5
declares rather than a hand-typed copy — and the alias is real, so a statement in the
module under test that had dropped its `lake.main.` qualifier would fail here rather
than only against Compose.
"""

from __future__ import annotations

from collections.abc import Iterator
from datetime import datetime
from typing import Any, Final

import duckdb
import pytest

from er.config.schema import Thresholds
from er.lake.model import REGISTRY, create_table_sql
from er.matching.edges import (
    CURRENT_EDGE_ORDER,
    EDGE_COLUMNS,
    MATCH_SCORES_RELATION,
    DuplicateEdgeKeyError,
    current_edges,
    current_edges_sql,
)
from er.matching.thresholds import is_auto_merge

#: The two keys these tests read under, and the second key that must not leak into the
#: first's result. Fixed strings rather than ULIDs: the assertions are about *which* key
#: a row carries, and a generated one would make a failure message unreadable.
MODEL_V1: Final = "v0001"
MODEL_V2: Final = "v0002"
TF_S1: Final = "01JWMDTFSNAP00000000000001"
TF_S2: Final = "01JWMDTFSNAP00000000000002"

#: S5's `match_scores` column list, in DDL order, read from the one place that declares
#: it. The inserts below are positional, so a relation that grew a column must fail the
#: length check in :func:`insert_edge` rather than shift every value one place.
MATCH_SCORE_COLUMNS: Final[tuple[str, ...]] = REGISTRY[MATCH_SCORES_RELATION].column_names

#: A record-key pair in canonical order (S5.0), and a second one for the tests that need
#: to see that a filter removed one pair and not the whole relation.
PAIR_AB: Final[tuple[str, str]] = ("crm:a", "crm:b")
PAIR_CD: Final[tuple[str, str]] = ("crm:c", "crm:d")

#: The gray band these tests threshold against. `auto_merge` is the clustering threshold
#: (S4.3) and is what S4.5.1 passes as `min_probability`.
THRESHOLDS: Final = Thresholds(auto_merge=0.95, review_low=0.7)

#: One ULP below `auto_merge`, which is the strongest form of "immediately below" a
#: `DOUBLE` column can carry. A round number like ``0.94`` would pass an implementation
#: that compared with `>` after rounding to two places.
JUST_BELOW_AUTO_MERGE: Final = 0.9499999999999999


@pytest.fixture
def lake() -> Iterator[duckdb.DuckDBPyConnection]:
    """An in-memory stand-in for the lake, holding S5's own `match_scores`."""
    connection = duckdb.connect()
    connection.execute("ATTACH ':memory:' AS lake")
    connection.execute(create_table_sql(REGISTRY[MATCH_SCORES_RELATION]))
    try:
        yield connection
    finally:
        connection.close()


def insert_edge(
    connection: duckdb.DuckDBPyConnection,
    pair: tuple[str, str],
    match_probability: float,
    *,
    model_version: str = MODEL_V1,
    tf_snapshot_id: str = TF_S1,
    is_active: bool = True,
    run_id: str = "01JWMDRN000000000000000001",
    scored_at: datetime = datetime(2026, 1, 1, 0, 0, 0),
) -> None:
    """Insert one `match_scores` row directly, positionally in S5's column order.

    Direct because the writer this relation has (`score_full`'s `MERGE INTO`, S4.3.4)
    cannot produce two of the rows these tests need, and because a scoring run would
    make the fixture the subject instead of the reader.
    """
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
        is_active,
        None,
        None,
        run_id,
        scored_at,
    )
    assert len(row) == len(MATCH_SCORE_COLUMNS), (
        f"{MATCH_SCORES_RELATION} declares {len(MATCH_SCORE_COLUMNS)} columns and this "
        f"helper supplies {len(row)}; the values are positional (S5)"
    )
    connection.execute(
        f"INSERT INTO lake.main.{MATCH_SCORES_RELATION} "
        f"({', '.join(MATCH_SCORE_COLUMNS)}) "
        f"VALUES ({', '.join('?' for _ in MATCH_SCORE_COLUMNS)})",
        list(row),
    )


def test_sql_selects_one_row_per_canonical_pair(lake: duckdb.DuckDBPyConnection) -> None:
    """AC1: one row per pair at the requested key, and no row from any other key.

    The same canonical pair is scored under two `(model_version, tf_snapshot_id)` keys —
    which is not a violated logical key but the normal state of a lake that has been
    re-scored under a new model or a new TF snapshot (S4.3.3). Reading either key must
    yield that key's probability and exactly one row.
    """
    insert_edge(lake, PAIR_AB, 0.99, model_version=MODEL_V1, tf_snapshot_id=TF_S1)
    insert_edge(lake, PAIR_AB, 0.51, model_version=MODEL_V2, tf_snapshot_id=TF_S2)
    insert_edge(lake, PAIR_CD, 0.97, model_version=MODEL_V1, tf_snapshot_id=TF_S1)

    assert current_edges(lake, MODEL_V1, TF_S1) == [(*PAIR_AB, 0.99), (*PAIR_CD, 0.97)]
    assert current_edges(lake, MODEL_V2, TF_S2) == [(*PAIR_AB, 0.51)]

    # The same claim about the SQL rather than about the helper: it is a standalone
    # statement, and it projects S4.5.1's three columns in S4.5.1's order.
    statement = current_edges_sql(MODEL_V1, TF_S1)
    result = lake.execute(statement)
    projected = tuple(column[0] for column in result.description or ())
    assert projected == EDGE_COLUMNS
    assert sorted(result.fetchall()) == [(*PAIR_AB, 0.99), (*PAIR_CD, 0.97)]

    # AC7 at this layer: the same arguments twice over an unchanged relation.
    assert current_edges(lake, MODEL_V1, TF_S1) == current_edges(lake, MODEL_V1, TF_S1)


def test_duplicate_key_raises_in_strict_mode(lake: duckdb.DuckDBPyConnection) -> None:
    """AC5's refusal arm: two rows on one logical key stop the read, naming the pair.

    The duplicate is planted with one row active and one invalidated, which is the shape
    a botched S4.5.5 invalidation produces and the shape a check filtered by `is_active`
    would wave through. S5.0 states the key unfiltered and S4.3.4 spells out "at most one
    row per key regardless of `is_active`", so the refusal must fire here too.
    """
    insert_edge(lake, PAIR_AB, 0.99, run_id="01JWMDRN000000000000000001")
    insert_edge(lake, PAIR_AB, 0.42, run_id="01JWMDRN000000000000000002", is_active=False)
    insert_edge(lake, PAIR_CD, 0.97)

    with pytest.raises(DuplicateEdgeKeyError) as refusal:
        current_edges(lake, MODEL_V1, TF_S1)

    message = str(refusal.value)
    assert PAIR_AB[0] in message and PAIR_AB[1] in message, (
        f"the refusal must name the offending pair, got {message!r}"
    )
    # A precondition failure (S4.0 exit 3): the fault is in whatever wrote the second
    # row, so re-running the reader cannot fix it.
    assert refusal.value.code == 3

    # The filtered forms must refuse too — a caller thresholding at `auto_merge` would
    # otherwise see a key that happens to have one row above the bound as well formed.
    with pytest.raises(DuplicateEdgeKeyError):
        current_edges(lake, MODEL_V1, TF_S1, min_probability=THRESHOLDS.auto_merge)
    with pytest.raises(DuplicateEdgeKeyError):
        current_edges(lake, MODEL_V1, TF_S1, include_inactive=True)

    # A key with no duplicate is unaffected: the refusal is per `(model_version,
    # tf_snapshot_id)`, not a property of the relation as a whole.
    assert current_edges(lake, MODEL_V2, TF_S2) == []


def test_non_strict_resolves_by_total_order(lake: duckdb.DuckDBPyConnection) -> None:
    """AC5's resolution arm: `strict=False` returns one row, chosen by the total order.

    The order is `(scored_at DESC, run_id DESC)`, so this asserts both terms: the
    most-recently-scored row wins outright, and when two rows carry the same `scored_at`
    the greater `run_id` wins. Asserting only the first term would pass an
    implementation that ordered by `scored_at` alone and returned an arbitrary row for
    the tie the second term exists to break.
    """
    assert CURRENT_EDGE_ORDER == ("scored_at DESC", "run_id DESC")

    later = datetime(2026, 3, 1, 12, 0, 0)
    earlier = datetime(2026, 1, 1, 0, 0, 0)
    insert_edge(lake, PAIR_AB, 0.60, run_id="01JWMDRN000000000000000009", scored_at=earlier)
    insert_edge(lake, PAIR_AB, 0.99, run_id="01JWMDRN000000000000000001", scored_at=later)

    # The later `scored_at` wins even though its `run_id` is the smaller of the two.
    assert current_edges(lake, MODEL_V1, TF_S1, strict=False) == [(*PAIR_AB, 0.99)]

    # Same instant, so the tie falls to `run_id DESC`.
    insert_edge(lake, PAIR_CD, 0.10, run_id="01JWMDRN000000000000000001", scored_at=later)
    insert_edge(lake, PAIR_CD, 0.80, run_id="01JWMDRN000000000000000002", scored_at=later)
    assert current_edges(lake, MODEL_V1, TF_S1, strict=False) == [
        (*PAIR_AB, 0.99),
        (*PAIR_CD, 0.80),
    ]


def test_min_probability_is_inclusive(lake: duckdb.DuckDBPyConnection) -> None:
    """AC3: `p >= min_probability`, so a pair at exactly `auto_merge` is an edge.

    Half-open the other way round would drop exactly the pairs S4.3 calls matches: the
    clustering threshold *is* `auto_merge`, and `is_auto_merge` is the predicate this
    bound has to agree with. The two are asserted against each other rather than against
    a transcribed number, because a bound written `>` here and `>=` there fragments an
    entity without failing anything.
    """
    insert_edge(lake, PAIR_AB, THRESHOLDS.auto_merge)
    insert_edge(lake, PAIR_CD, JUST_BELOW_AUTO_MERGE)

    assert is_auto_merge(THRESHOLDS.auto_merge, THRESHOLDS)
    assert not is_auto_merge(JUST_BELOW_AUTO_MERGE, THRESHOLDS)

    kept = current_edges(lake, MODEL_V1, TF_S1, min_probability=THRESHOLDS.auto_merge)

    assert kept == [(*PAIR_AB, THRESHOLDS.auto_merge)]
    assert [edge for edge in kept if not is_auto_merge(edge[2], THRESHOLDS)] == [], (
        "the bound and is_auto_merge must select the same rows (S4.3, S4.5.1)"
    )
    # Without the bound both pairs are edges, so the exclusion above is the bound's
    # doing and not the fixture's.
    assert len(current_edges(lake, MODEL_V1, TF_S1)) == 2

    # A bound that is not a probability empties the edge set silently rather than
    # failing, so it is refused instead.
    for invalid in (float("nan"), float("inf"), -0.5, 1.5):
        with pytest.raises(ValueError):
            current_edges_sql(MODEL_V1, TF_S1, min_probability=invalid)
