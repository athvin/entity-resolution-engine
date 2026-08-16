"""The half-open gray band and the probability/weight conversion (S4.3, MINOR-thresholds).

Two sentences of S4.3 are under test here, and both are the kind that a wrong
implementation satisfies loudly nowhere and quietly everywhere:

* "Where a Splink call takes a match **weight**, pass ``log2(p/(1-p))``; never rely on
  Splink's ``-4`` default." The base of the logarithm is the whole content of the
  first clause — ``ln`` instead of ``log2`` is a threshold about ``0.69`` weights too
  low, which is a probability of ``0.44`` where the config said ``0.60`` — and the
  second clause names a default that corresponds to a probability of about ``0.059``,
  far below any `review_low` a document would set. :func:`test_prob_to_weight_round_trip`
  pins the base against the arithmetic and the default against `configs/test.yaml`.

* "The gray band is **half-open**: ``review_low <= p < auto_merge``." Both boundaries
  are asserted, in both predicates, because the failure of a closed band is silent: a
  pair at exactly `auto_merge` queued for review is a steward being asked about an edge
  that clustering has already merged (S4.3, S4.3.5).

The document is the shipped `configs/test.yaml` rather than a hand-built one: its
``0.60``/``0.95`` are the two probabilities AC6 names, and a test that invented its own
thresholds would not be checking the pair the rest of the suite scores against.
"""

from __future__ import annotations

from math import log2
from pathlib import Path

import pytest

from er.config.loader import load_config
from er.config.schema import Thresholds
from er.matching.thresholds import (
    SPLINK_DEFAULT_MATCH_WEIGHT,
    in_gray_band,
    is_auto_merge,
    prob_to_weight,
    weight_to_prob,
)

REPO_ROOT = Path(__file__).resolve().parents[3]
TEST_CONFIG = REPO_ROOT / "configs" / "test.yaml"

#: AC6's two probabilities. They are `review_low` and `auto_merge` of the shipped
#: document, asserted to be so below rather than assumed.
REVIEW_LOW = 0.60
AUTO_MERGE = 0.95

#: The tolerance AC6 states. Tight on purpose: the conversion is closed-form, so
#: anything looser would accept a different formula.
TOLERANCE = 1e-12


@pytest.fixture(scope="module")
def thresholds() -> Thresholds:
    """The `thresholds:` block of the document CI scores against (S6)."""
    return load_config(TEST_CONFIG).thresholds


def test_prob_to_weight_round_trip(thresholds: Thresholds) -> None:
    """AC6: the conversion is `log2(p/(1-p))`, and `weight_to_prob` inverts it."""
    assert (thresholds.review_low, thresholds.auto_merge) == (REVIEW_LOW, AUTO_MERGE), (
        "configs/test.yaml no longer carries the thresholds this test pins"
    )

    for probability in (REVIEW_LOW, AUTO_MERGE):
        expected = log2(probability / (1.0 - probability))
        weight = prob_to_weight(probability)
        assert abs(weight - expected) < TOLERANCE, f"{probability} rendered as {weight}"
        assert abs(weight_to_prob(weight) - probability) < TOLERANCE

    # The second half of the S4.3 sentence: Splink's default is not merely a different
    # number, it is a threshold BELOW `review_low`, so a pass that relied on it would
    # persist pairs S4.3.4 says are never written.
    assert weight_to_prob(SPLINK_DEFAULT_MATCH_WEIGHT) < thresholds.review_low
    assert prob_to_weight(thresholds.review_low) > SPLINK_DEFAULT_MATCH_WEIGHT


def test_prob_to_weight_refuses_the_infinite_endpoints() -> None:
    """`p = 0` and `p = 1` have no finite weight, and `auto_merge` may be exactly 1.0.

    An unrefused endpoint would hand Splink an infinity, and a threshold of `inf`
    scores nothing at all — a full run that wrote zero rows and failed nothing.
    """
    for probability in (0.0, 1.0, -0.5, 1.5):
        with pytest.raises(ValueError, match="0 < p < 1"):
            prob_to_weight(probability)


def test_gray_band_boundaries_are_half_open(thresholds: Thresholds) -> None:
    """AC5: `review_low` is in the band, `auto_merge` is not, and nothing is both."""
    assert in_gray_band(thresholds.review_low, thresholds)
    assert not in_gray_band(thresholds.auto_merge, thresholds)
    assert not is_auto_merge(thresholds.review_low, thresholds)
    assert is_auto_merge(thresholds.auto_merge, thresholds)

    # Below the band is neither, and the two predicates partition everything at or
    # above `review_low` — which is exactly the set this stage persists (S4.3.4).
    below = thresholds.review_low / 2
    assert not in_gray_band(below, thresholds)
    assert not is_auto_merge(below, thresholds)
    for probability in (thresholds.review_low, 0.80, thresholds.auto_merge, 1.0):
        assert in_gray_band(probability, thresholds) != is_auto_merge(probability, thresholds), (
            f"{probability} is in both the gray band and the merge set, or in neither"
        )
