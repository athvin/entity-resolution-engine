"""The S4.3.2 activation guard as a pure predicate (S8.4).

Hand-built edge rows, no lake: the guard's whole design is that the SQL half only
selects current-per-pair rows and every decision — the `review_low` boundary, the
generation count, the refusal — lives in a function this file can corner.
"""

from __future__ import annotations

import pytest

from er.entities.guards import (
    GenerationRow,
    MixedScoringGenerationError,
    assert_single_scoring_generation,
)

REVIEW_LOW = 0.60


def row(
    pair: tuple[str, str],
    model_version: str,
    tf_snapshot_id: str,
    probability: float,
) -> GenerationRow:
    return (pair[0], pair[1], model_version, tf_snapshot_id, probability)


def test_guard_is_pure_over_edge_rows() -> None:
    """AC6: zero rows pass, one generation passes, two raise with both named."""
    assert_single_scoring_generation([], review_low=REVIEW_LOW)

    one_generation = [
        row(("s:a", "s:b"), "v0001", "SNAP1", 0.99),
        row(("s:a", "s:c"), "v0001", "SNAP1", 0.72),
    ]
    assert_single_scoring_generation(one_generation, review_low=REVIEW_LOW)

    mixed_model = [
        row(("s:a", "s:b"), "v0001", "SNAP1", 0.99),
        row(("s:c", "s:d"), "v0002", "SNAP1", 0.98),
    ]
    with pytest.raises(MixedScoringGenerationError) as refused:
        assert_single_scoring_generation(mixed_model, review_low=REVIEW_LOW)
    message = str(refused.value)
    assert "v0001" in message and "v0002" in message, message
    assert "er match --mode full" in message, message
    assert refused.value.generations == {("v0001", "SNAP1"): 1, ("v0002", "SNAP1"): 1}

    # The tf arm is the same hazard with the other key half (S4.3.3).
    mixed_snapshot = [
        row(("s:a", "s:b"), "v0001", "SNAP1", 0.99),
        row(("s:c", "s:d"), "v0001", "SNAP2", 0.98),
    ]
    with pytest.raises(MixedScoringGenerationError) as tf_refused:
        assert_single_scoring_generation(mixed_snapshot, review_low=REVIEW_LOW)
    assert "SNAP1" in str(tf_refused.value) and "SNAP2" in str(tf_refused.value)


def test_rows_below_review_low_are_ignored() -> None:
    """AC6: a second generation entirely under the threshold is not a scale in play."""
    harmless = [
        row(("s:a", "s:b"), "v0001", "SNAP1", 0.99),
        row(("s:c", "s:d"), "v0002", "SNAP1", 0.59),
    ]
    assert_single_scoring_generation(harmless, review_low=REVIEW_LOW)

    # The boundary is inclusive on the dangerous side: AT review_low is in play,
    # matching S4.3.5's half-open band starting at the threshold.
    at_threshold = [
        row(("s:a", "s:b"), "v0001", "SNAP1", 0.99),
        row(("s:c", "s:d"), "v0002", "SNAP1", REVIEW_LOW),
    ]
    with pytest.raises(MixedScoringGenerationError):
        assert_single_scoring_generation(at_threshold, review_low=REVIEW_LOW)
