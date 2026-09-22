"""The S4.3.2 activation guard: one scoring generation per reconcile (S4.3.3, S4.7).

Activating a new `model_version` — or freezing a new `tf_snapshot_id` (S4.3.3) —
without a full rescore leaves `match_scores` speaking two probability scales, and a
reconcile that clustered them would compare both against one `auto_merge` as if
they were commensurable. INV-SCORE makes a probability a pure function of the
`(model_version, tf_snapshot_id)` pair among its inputs, so rows from two pairs are
two different experiments; S4.3.2 therefore requires the full rescore FIRST, and
this module is the check that makes the requirement a refusal rather than advice.

Two properties are load-bearing:

* **The guard reads the CURRENT row per canonical pair, never the raw cumulative
  table.** `match_scores`' MERGE key includes the generation, so a correction pass
  leaves every pair's old-generation row in place forever — active, above
  `review_low`, and permanently mixed at the table level. What matters is what a
  clustering would actually consume: the newest row per pair. A mixed *current*
  set means some pairs would cluster on the old scale and some on the new, which
  is exactly the hazard.
* **The threshold is `review_low`, not `auto_merge` (S4.3.2).** The gray band is
  part of the run's evidence — it feeds `review_queue` and a steward's decision —
  so a generation that only shows up between the thresholds is still a second
  scale in play.

Assertion-sourced edges can never confound this guard: S4.4 forbids persisting
them to `match_scores` at all, which is why `model_version` is NOT NULL there and
why no NULL-generation branch exists here. A test pins that reasoning.
"""

from __future__ import annotations

from collections.abc import Iterable
from typing import Final

import duckdb

from er.errors import PreconditionFailure
from er.lake.model import SCHEMA_QUALIFIER

__all__ = [
    "GENERATION_ROW_COLUMNS",
    "GenerationRow",
    "MixedScoringGenerationError",
    "assert_single_scoring_generation",
    "scoring_generation_rows",
]

#: One active current-per-pair row, in the shape the pure guard consumes:
#: the canonical pair, its scoring generation, and the probability.
GenerationRow = tuple[str, str, str, str, float]

GENERATION_ROW_COLUMNS: Final[tuple[str, ...]] = (
    "rec_a_key",
    "rec_b_key",
    "model_version",
    "tf_snapshot_id",
    "match_probability",
)

_MATCH_SCORES: Final = f"{SCHEMA_QUALIFIER}.match_scores"

#: The newest row per pair, by the same total order `er.matching.edges` resolves
#: with: scored_at, then run_id, so two rows stamped in one instant still order.
_CURRENT_PER_PAIR_SQL: Final = f"""
SELECT rec_a_key, rec_b_key, model_version, tf_snapshot_id, match_probability
  FROM {_MATCH_SCORES}
 WHERE is_active
 QUALIFY row_number() OVER (
     PARTITION BY rec_a_key, rec_b_key
     ORDER BY scored_at DESC, run_id DESC
 ) = 1
"""


class MixedScoringGenerationError(PreconditionFailure):
    """Two `(model_version, tf_snapshot_id)` generations above `review_low` (S4.3.2).

    A precondition (exit ``3``, `error_class='precondition'`), never a warning and
    never exit ``1``: the run is refused before clustering, so nothing is written
    and the operator's next action — `er match --mode full` at the active model —
    is named in the message.
    """

    def __init__(self, generations: dict[tuple[str, str], int]) -> None:
        named = ", ".join(
            f"(model_version={model!r}, tf_snapshot_id={snapshot!r}: {count} pair(s))"
            for (model, snapshot), count in sorted(generations.items())
        )
        super().__init__(
            f"the current edge set above review_low carries {len(generations)} scoring "
            f"generations: {named}. S4.3.2 requires a full rescore at the active model "
            f"before the next reconcile — run `er match --mode full` and re-run."
        )
        self.generations = generations


def scoring_generation_rows(connection: duckdb.DuckDBPyConnection) -> list[GenerationRow]:
    """The active current-per-pair rows, with their generations, unthresholded.

    Unthresholded on purpose: the threshold is the PURE guard's job, so the unit
    layer can prove the `review_low` boundary without a lake.
    """
    return [
        (str(rec_a), str(rec_b), str(model), str(snapshot), float(probability))
        for rec_a, rec_b, model, snapshot, probability in connection.execute(
            _CURRENT_PER_PAIR_SQL
        ).fetchall()
    ]


def assert_scoring_generation(connection: duckdb.DuckDBPyConnection, *, review_low: float) -> None:
    """Check current generations in SQL, returning only one count per generation."""
    rows = connection.execute(
        f"SELECT model_version, tf_snapshot_id, count(*) FROM ({_CURRENT_PER_PAIR_SQL}) "
        "WHERE match_probability >= ? AND NOT isnan(match_probability) "
        "GROUP BY model_version, tf_snapshot_id",
        [review_low],
    ).fetchall()
    if len(rows) > 1:
        raise MixedScoringGenerationError(
            {(str(model), str(snapshot)): int(count) for model, snapshot, count in rows}
        )


def assert_single_scoring_generation(
    rows: Iterable[GenerationRow],
    *,
    review_low: float,
) -> None:
    """Raise unless at most one generation scores at or above ``review_low``.

    Pure: no connection, no config, no clock — testable over hand-built rows
    (S8.4). Zero rows pass (a first run has nothing to cluster and nothing to
    confuse); one generation passes; two raise with both named and counted.

    Args:
        rows: active current-per-pair rows, from
            :func:`scoring_generation_rows` or a test's hand-built list.
        review_low: the S6 threshold; rows strictly below it are ignored, because
            a row no clustering and no review queue will ever read is not a scale
            in play (S4.3.2 pins `review_low`, not `auto_merge`).

    Raises:
        MixedScoringGenerationError: more than one distinct
            `(model_version, tf_snapshot_id)` at or above the threshold.
    """
    generations: dict[tuple[str, str], int] = {}
    for _rec_a, _rec_b, model_version, tf_snapshot_id, probability in rows:
        if probability >= review_low:
            key = (model_version, tf_snapshot_id)
            generations[key] = generations.get(key, 0) + 1
    if len(generations) > 1:
        raise MixedScoringGenerationError(generations)
