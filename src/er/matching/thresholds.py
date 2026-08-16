"""The gray band and the probability/weight conversion — declared exactly once (S4.3).

S4.3 opens with two sentences that every scoring path in the repository depends on:
"All config thresholds are **probabilities**. Where a Splink call takes a match
**weight**, pass ``log2(p/(1-p))``; never rely on Splink's ``-4`` default", and "The
gray band is **half-open**: ``review_low <= p < auto_merge``". This module is the only
place either sentence is turned into code.

Both are the kind of rule that a second copy silently breaks. A gray band written
``review_low <= p <= auto_merge`` at one call site queues a pair for review that the
clustering threshold has already merged — the steward is asked about an edge the
partition has decided — and nothing fails: the counters still add up, and only the
review queue is wrong. A weight computed with the natural logarithm instead of
``log2`` is a threshold roughly ``0.69`` weights too low, which admits pairs below
``review_low`` into the scored set and makes AC1's "zero rows fall below `review_low`"
false for a reason no reader would look for. So the four functions below are imported
rather than reimplemented: `--mode full` (this milestone) and the S4.3.4 two-pass
incremental scorer (ER-065) both read the band from here, and so does clustering,
whose threshold **is** ``auto_merge``.

**Why the conversion exists at all.** Splink's inference surface is inconsistent about
units: ``predict()`` takes ``threshold_match_probability``, while
``find_matches_to_new_records()`` takes ``match_weight_threshold`` and *defaults it to*
:data:`SPLINK_DEFAULT_MATCH_WEIGHT` — a weight of ``-4``, i.e. a probability of about
``0.059``. A pass that omits the argument therefore does not score "everything"; it
scores everything above a threshold nobody chose, and persists pairs S4.3.4 says are
never written. :func:`prob_to_weight` is what a caller passes instead, and
:func:`weight_to_prob` is its exact inverse, so a threshold can be read back in the
units the config states it in.
"""

from __future__ import annotations

from math import exp2, log2
from typing import Final

from er.config.schema import Thresholds

__all__ = [
    "SPLINK_DEFAULT_MATCH_WEIGHT",
    "in_gray_band",
    "is_auto_merge",
    "prob_to_weight",
    "weight_to_prob",
]

#: Splink's ``match_weight_threshold`` default, in weights. Named here so that "never
#: rely on the -4 default" (S4.3) is a value a test can assert a threshold is NOT,
#: rather than a number a reader has to go and find in Splink's signature.
SPLINK_DEFAULT_MATCH_WEIGHT: Final[float] = -4.0


def prob_to_weight(probability: float) -> float:
    """``log2(p/(1-p))`` — the match weight of a match probability (S4.3).

    Args:
        probability: a probability strictly between 0 and 1.

    Returns:
        The Bayes factor of ``probability``, in bits: the units every Splink
        ``match_weight`` argument is stated in.

    Raises:
        ValueError: ``probability`` is outside the open interval ``(0, 1)``, where the
            weight is infinite. Both endpoints are reachable from a valid S6 document —
            ``auto_merge`` may be exactly ``1.0`` — and an infinite threshold silently
            scores nothing, so the refusal is explicit rather than an ``inf`` handed on
            to Splink. Note that `review_low`, the one threshold this stage converts,
            is always strictly below ``auto_merge <= 1`` and therefore always valid.
    """
    if not 0.0 < probability < 1.0:
        raise ValueError(
            f"a match weight is log2(p/(1-p)) and is defined for 0 < p < 1, got {probability!r}; "
            f"p=0 and p=1 are the infinite weights, which no Splink threshold can express"
        )
    return log2(probability / (1.0 - probability))


def weight_to_prob(weight: float) -> float:
    """The match probability of a match weight: the exact inverse of :func:`prob_to_weight`.

    Written as ``1 / (1 + 2**-w)`` rather than ``2**w / (1 + 2**w)``: the two agree
    mathematically, but the second overflows for a large positive weight, and a large
    positive weight is exactly what a near-certain pair has.
    """
    return 1.0 / (1.0 + exp2(-weight))


def in_gray_band(probability: float, thresholds: Thresholds) -> bool:
    """Whether ``probability`` falls in the half-open gray band (S4.3, S4.3.5).

    ``review_low <= p < auto_merge``. Half-open in both directions on purpose: a pair
    at exactly ``auto_merge`` is a match and is clustered, and a pair at exactly
    ``review_low`` is the lowest thing this stage persists and is a steward's to
    decide. The two boundaries are therefore never both true and never both false —
    :func:`is_auto_merge` is the complement of this predicate above ``review_low``.
    """
    return thresholds.review_low <= probability < thresholds.auto_merge


def is_auto_merge(probability: float, thresholds: Thresholds) -> bool:
    """Whether ``probability`` is a match: ``p >= auto_merge`` (S4.3).

    The clustering threshold **is** ``auto_merge`` (S4.3), so this predicate and the
    edge set S4.5 clusters are the same claim under two spellings.
    """
    return probability >= thresholds.auto_merge
