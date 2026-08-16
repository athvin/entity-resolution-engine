"""The S4.3.5 `review_queue` upsert, without Docker (S4.3.5, S4.4.2, S5.0, S11).

The upsert is the whole reason the table is idempotent, and it has three outcomes
rather than two — insert, refresh, skip — so what is asserted here is each of them
against the state the previous call left behind:

* a new subject lands `open` with `first_seen_run_id == last_seen_run_id` and a
  `waterfall` that still carries the `gamma_*` vector and the per-comparison Bayes
  factors `predict()` produced (S4.3.5);
* a second run over the same subject moves `last_seen_run_id` and NOTHING else —
  not the row count, not `first_seen_run_id`, not the payload;
* a subject a steward has settled is skipped outright, `last_seen_run_id`
  included, because a dismissal that kept being refreshed would decay into a row
  that looks freshly seen;
* and `reason` is part of a row's identity, so one canonical pair open for
  `gray_band` and escalated as `never_unsatisfiable` is two rows and two steward
  tasks (S5.0, M20).

`lake` is an in-memory database ATTACHed under the alias and holding S5's own
`review_queue`, exactly as `tests/unit/review/test_assertions_model.py` does it:
every statement in the module under test is written `lake.main.…` (S4.0b forbids
DuckLake being the default catalog), and a fixture that made the table reachable
unqualified would let a missing qualifier pass here and fail against a real lake.

Ids come from :class:`~er.entities.ids.CountingIdFactory`, so a `review_id` is the
same string in every process (S4.5.4, D10).
"""

from __future__ import annotations

import json
from collections.abc import Iterator
from typing import Any, Final

import duckdb
import pytest

from er.entities.ids import CountingIdFactory
from er.errors import ExitCode, StageFailure, exit_code_for
from er.lake.model import REGISTRY, SCHEMA_QUALIFIER, create_table_sql
from er.review.queue import (
    COHERENCE,
    DISMISSED,
    ENTITY,
    GRAY_BAND,
    NEVER_UNSATISFIABLE,
    OPEN,
    PAIR,
    RESOLVED_MATCH,
    RESOLVED_NO_MATCH,
    REVIEW_QUEUE_RELATION,
    GrayBandPair,
    upsert_entity_finding,
    upsert_escalation,
    upsert_gray_band_pairs,
)

REVIEW_QUEUE: Final = f"{SCHEMA_QUALIFIER}.{REVIEW_QUEUE_RELATION}"

#: Two record keys given to every upsert the WRONG way round — `webforms:9` sorts
#: after `crm:1` — because "canonicalised on write" (S5.0, D9) is only observable
#: against a caller that got the order wrong.
KEY_A: Final = "webforms:9"
KEY_B: Final = "crm:1"
CANONICAL: Final = (KEY_B, KEY_A)

#: Two runs, so "refreshes `last_seen_run_id`" is a change between two known values.
RUN_1: Final = "01JQZ8XKQ4T7VN3M2B9CDEFGH1"
RUN_2: Final = "01JQZ8XKQ4T7VN3M2B9CDEFGH2"

#: One `predict()` evidence payload: a `gamma_*` per comparison and the Bayes factor
#: beside it, which is exactly what S4.3.5 says MUST be retained rather than
#: projected away. `match_weight` rides along because the real payload carries it and
#: the store is verbatim.
WATERFALL: Final[dict[str, Any]] = {
    "gamma_email": 2,
    "gamma_family_name": 1,
    "gamma_birth_date": 0,
    "bf_email": 41.7,
    "bf_family_name": 3.25,
    "bf_birth_date": 0.11,
    "match_weight": 1.9,
}


@pytest.fixture
def lake() -> Iterator[duckdb.DuckDBPyConnection]:
    """An in-memory stand-in for the lake, holding S5's own `review_queue`."""
    connection = duckdb.connect()
    connection.execute("ATTACH ':memory:' AS lake")
    connection.execute(create_table_sql(REGISTRY[REVIEW_QUEUE_RELATION]))
    try:
        yield connection
    finally:
        connection.close()


@pytest.fixture
def ids() -> CountingIdFactory:
    """The same id sequence in every process (S4.5.4, D10)."""
    return CountingIdFactory(start=1)


def gray_band_pair(probability: float = 0.9) -> GrayBandPair:
    """One scored pair in the band, keys deliberately un-canonical."""
    return GrayBandPair(
        rec_a_key=KEY_A,
        rec_b_key=KEY_B,
        match_probability=probability,
        waterfall=WATERFALL,
    )


def raw_rows(connection: duckdb.DuckDBPyConnection) -> list[tuple[Any, ...]]:
    """Every row as the engine holds it, in `review_id` order.

    Whole tuples rather than named columns: "byte-unchanged" is a claim about the
    row, and a projection is free to omit the column that moved.
    """
    columns = ", ".join(REGISTRY[REVIEW_QUEUE_RELATION].column_names)
    return [
        tuple(row)
        for row in connection.execute(
            f"SELECT {columns} FROM {REVIEW_QUEUE} ORDER BY review_id"
        ).fetchall()
    ]


def resolve_in_place(connection: duckdb.DuckDBPyConnection, review_id: str, status: str) -> None:
    """Settle a row the way `er review resolve` does, without the CLI in the way.

    Written here rather than through :func:`~er.review.queue.resolve_review` on
    purpose: the subject of these tests is the UPSERT's reaction to a settled row,
    and routing through the resolver would make a failure ambiguous between the two.
    """
    connection.execute(
        f"UPDATE {REVIEW_QUEUE} SET status = ?, resolved_by = 'tester', "
        f"resolved_at = TIMESTAMP '2026-01-01 12:00:00' WHERE review_id = ?",
        [status, review_id],
    )


def test_insert_then_refresh_last_seen_only(
    lake: duckdb.DuckDBPyConnection, ids: CountingIdFactory
) -> None:
    """AC1: a new pair inserts one open row; a second run moves `last_seen_run_id` alone."""
    first = upsert_gray_band_pairs(lake, [gray_band_pair()], run_id=RUN_1, id_factory=ids)

    assert (first.added, first.refreshed_count) == (1, 0)
    (row,) = first.inserted
    assert (row.subject_type, row.reason, row.status) == (PAIR, GRAY_BAND, OPEN)
    # Canonicalised on write, however the caller ordered the arguments (S5.0, D9).
    assert row.pair == CANONICAL
    assert row.entity_id is None
    assert row.first_seen_run_id == row.last_seen_run_id == RUN_1
    assert row.match_probability == pytest.approx(0.9)

    stored = raw_rows(lake)
    assert len(stored) == 1

    second = upsert_gray_band_pairs(lake, [gray_band_pair()], run_id=RUN_2, id_factory=ids)

    # Refreshed, not inserted: the row count is unchanged and the id is the first one.
    assert (second.added, second.refreshed_count) == (0, 1)
    assert second.refreshed[0].review_id == row.review_id
    assert len(raw_rows(lake)) == 1
    first_seen, last_seen, status = lake.execute(
        f"SELECT first_seen_run_id, last_seen_run_id, status FROM {REVIEW_QUEUE}"
    ).fetchall()[0]
    # `first_seen_run_id` is written once and never updated (S4.3.5): it is how long
    # the steward task has been waiting, and a refresh that moved it would erase that.
    assert (first_seen, last_seen, status) == (RUN_1, RUN_2, OPEN)


@pytest.mark.parametrize("settled", [RESOLVED_MATCH, RESOLVED_NO_MATCH, DISMISSED])
def test_resolved_subject_is_skipped_not_refreshed(
    lake: duckdb.DuckDBPyConnection, ids: CountingIdFactory, settled: str
) -> None:
    """AC2: a settled subject is skipped outright — no insert, and no refresh either.

    All three settled statuses, because the skip clause of S4.3.5 names all three and
    a rule that only recognised `dismissed` would let a resolved pair resurface on the
    next run as though the steward had never answered.
    """
    (row,) = upsert_gray_band_pairs(lake, [gray_band_pair()], run_id=RUN_1, id_factory=ids).inserted
    resolve_in_place(lake, row.review_id, settled)
    before = raw_rows(lake)

    result = upsert_gray_band_pairs(lake, [gray_band_pair()], run_id=RUN_2, id_factory=ids)

    assert (result.added, result.refreshed_count) == (0, 0)
    assert [skipped.review_id for skipped in result.skipped] == [row.review_id]
    # Byte-unchanged, whole row: `last_seen_run_id` is what a refresh would have
    # moved, and a dismissal whose `last_seen_run_id` kept advancing would decay
    # into a row that looks freshly seen every run.
    assert raw_rows(lake) == before


def test_two_reasons_for_one_pair_are_two_rows(
    lake: duckdb.DuckDBPyConnection, ids: CountingIdFactory
) -> None:
    """AC3: `reason` is in the open-row key, so an escalation is a second row (S5.0).

    The two reasons are independent steward tasks — S4.3.5's gray band says "this
    score is uncertain", S4.4.2's escalation says "this `never` cannot be satisfied
    by cutting" — and collapsing them into one row would drop one of them (M20).
    """
    gray = upsert_gray_band_pairs(lake, [gray_band_pair()], run_id=RUN_1, id_factory=ids)
    escalated = upsert_escalation(
        lake, rec_a_key=KEY_A, rec_b_key=KEY_B, run_id=RUN_1, id_factory=ids
    )

    assert escalated.added == 1
    rows = lake.execute(
        f"SELECT rec_a_key, rec_b_key, reason, status, waterfall FROM {REVIEW_QUEUE} "
        f"ORDER BY reason"
    ).fetchall()
    assert [(a, b, reason, status) for a, b, reason, status, _ in rows] == [
        (*CANONICAL, GRAY_BAND, OPEN),
        (*CANONICAL, NEVER_UNSATISFIABLE, OPEN),
    ]
    # An escalation is a partition finding reached after clustering, not a scored
    # pair, so it carries no comparison vector and S5 makes the column nullable.
    assert rows[1][4] is None

    # Each reason refreshes only its own row, which is the same statement the open-row
    # key makes: the two rows are two subjects that happen to share a pair.
    again = upsert_gray_band_pairs(lake, [gray_band_pair()], run_id=RUN_2, id_factory=ids)
    assert [refreshed.review_id for refreshed in again.refreshed] == [gray.inserted[0].review_id]
    assert len(raw_rows(lake)) == 2


def test_waterfall_retains_gamma_and_bayes_factors(
    lake: duckdb.DuckDBPyConnection, ids: CountingIdFactory
) -> None:
    """AC1: the `predict()` payload survives the round trip, and a stripped one is refused.

    S4.3.5 says the `gamma_*` comparison vector and the per-comparison Bayes factors
    "MUST be retained rather than projected away", so both halves are asserted: what
    comes back out of the relation, and that a payload missing either family is
    refused at the write rather than discovered by a steward staring at an empty
    waterfall.
    """
    upsert_gray_band_pairs(lake, [gray_band_pair()], run_id=RUN_1, id_factory=ids)

    (stored,) = lake.execute(f"SELECT waterfall FROM {REVIEW_QUEUE}").fetchall()
    payload = json.loads(str(stored[0]))
    assert payload == WATERFALL
    assert {key for key in payload if key.startswith("gamma_")} == {
        "gamma_email",
        "gamma_family_name",
        "gamma_birth_date",
    }
    assert {key for key in payload if key.startswith("bf_")} == {
        "bf_email",
        "bf_family_name",
        "bf_birth_date",
    }

    stripped = GrayBandPair(
        rec_a_key=KEY_A,
        rec_b_key=KEY_B,
        match_probability=0.9,
        waterfall={key: value for key, value in WATERFALL.items() if not key.startswith("bf_")},
    )
    with pytest.raises(StageFailure) as refused:
        upsert_gray_band_pairs(lake, [stripped], run_id=RUN_2, id_factory=ids)

    assert exit_code_for(refused.value) == int(ExitCode.STAGE_FAILURE)
    assert "bf_* key" in str(refused.value)
    # The refusal wrote nothing: the only row is still the one that was accepted.
    assert len(raw_rows(lake)) == 1


def test_entity_finding_has_no_pair(
    lake: duckdb.DuckDBPyConnection, ids: CountingIdFactory
) -> None:
    """AC7: an S11 coherence finding is an entity subject with both keys NULL.

    Here rather than only in the CLI suite because the *shape* of the row is what
    makes `resolve --as match` illegal against it (S11): there is no pair to assert
    over, and that is a property of the writer rather than of the command.
    """
    result = upsert_entity_finding(
        lake, entity_id="01JQZ8XKQ4T7VN3M2B9CDEFGHE", run_id=RUN_1, id_factory=ids
    )

    (row,) = result.inserted
    assert (row.subject_type, row.reason, row.status) == (ENTITY, COHERENCE, OPEN)
    assert row.entity_id == "01JQZ8XKQ4T7VN3M2B9CDEFGHE"
    assert row.pair is None
    assert (row.rec_a_key, row.rec_b_key) == (None, None)

    # The same subject twice is one row: NULL record keys must not defeat the
    # open-row key, which is what `IS NOT DISTINCT FROM` is there for.
    again = upsert_entity_finding(
        lake, entity_id="01JQZ8XKQ4T7VN3M2B9CDEFGHE", run_id=RUN_2, id_factory=ids
    )
    assert (again.added, again.refreshed_count) == (0, 1)
    assert len(raw_rows(lake)) == 1
