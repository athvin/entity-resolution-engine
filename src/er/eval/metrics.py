"""The single quality-metric implementation of S8.5 (DesignDoc.md S8.5, S5.0).

Every match-quality number in the repository — T-MATCH-1a, T-MATCH-1b, the
benchmark's quality block — is computed by :func:`pairwise_metrics` and by nothing
else. `scripts/lint_metrics.py` runs in the static job (S9.1) and fails on a second
implementation, so the function's conventions are load-bearing for every caller and
are pinned here rather than left to each call site:

* **Empty `predicted` yields precision `1.0`.** Vacuously: with `tp = fp = 0` there
  is no asserted pair to be wrong about. A `0.0` here would make a scorer that
  correctly asserts nothing on an empty batch read as maximally imprecise.
* **Empty `truth` yields recall `1.0`.** The same vacuity on the other side: there
  is nothing to have missed.
* **`f1` is `0.0` when `precision + recall == 0`.** The harmonic mean's own limit,
  stated rather than left to a ZeroDivisionError.
* **Everything is evaluated strictly within `universe`, and leaving it raises.**
  S8.5's three metric rows differ only in which universe they are computed over,
  and that discipline is what makes edge-level numbers comparable across runs: a
  predicted or truth pair outside the universe is a category error by the caller —
  a blocked-set metric fed a full-corpus pair — not a data point to coerce.
* **Every pair is canonical (S5.0).** `rec_a_key < rec_b_key`, the orientation
  `match_scores` and every helper share; the reversed pair raises so a set built
  the other way round fails loudly instead of intersecting as disjoint.
"""

from __future__ import annotations

from collections.abc import Iterable
from collections.abc import Set as AbstractSet
from dataclasses import dataclass

__all__ = [
    "PairwiseMetrics",
    "cluster_closure_pairs",
    "membership_partition",
    "pairwise_metrics",
]

#: A canonical pair: `rec_a_key < rec_b_key` (S5.0).
Pair = tuple[str, str]


@dataclass(frozen=True, slots=True)
class PairwiseMetrics:
    """Precision, recall and f1, with the raw counts they were computed from.

    The counts travel with the ratios because T-MATCH-1a gates on ABSOLUTE counts
    (S8.3: "false-positive pairs == 0 and missed true pairs <= 1"), and a caller
    re-deriving `fp` from `precision` would divide the convention back out.
    """

    tp: int
    fp: int
    fn: int
    precision: float
    recall: float
    f1: float


def membership_partition(rows: Iterable[tuple[str, str]]) -> dict[str, frozenset[str]]:
    """`entity_id -> members` from `(record_key, entity_id)` membership rows.

    The partition S8.5's cluster-level row is stated over. A record appearing
    under two entities raises: `entity_membership` is CURRENT STATE with exactly
    one row per record (S4.5.3), and a duplicate here means the caller read
    something else.
    """
    seen: dict[str, str] = {}
    grouped: dict[str, set[str]] = {}
    for record_key, entity_id in rows:
        previous = seen.get(record_key)
        if previous is not None and previous != entity_id:
            raise ValueError(
                f"{record_key!r} appears under two entities ({previous!r}, "
                f"{entity_id!r}); entity_membership holds one row per record (S4.5.3)"
            )
        seen[record_key] = entity_id
        grouped.setdefault(entity_id, set()).add(record_key)
    return {entity_id: frozenset(members) for entity_id, members in grouped.items()}


def cluster_closure_pairs(rows: Iterable[tuple[str, str]]) -> set[Pair]:
    """The transitive closure of a membership as canonical pairs (S8.5, S5.0).

    S8.5's cluster-level `predicted` set: every within-entity pair, each exactly
    once, in the `rec_a_key < rec_b_key` orientation. Computed here — beside
    :func:`pairwise_metrics` — so T-MATCH-1b and the benchmark's quality block
    read one closure rather than growing a second that disagrees about ordering.
    """
    pairs: set[Pair] = set()
    for members in membership_partition(rows).values():
        ordered = sorted(members)
        for index, rec_a_key in enumerate(ordered):
            for rec_b_key in ordered[index + 1 :]:
                pairs.add((rec_a_key, rec_b_key))
    return pairs


def _require_canonical(name: str, pairs: AbstractSet[Pair]) -> None:
    """Every pair in ``pairs`` is `(a, b)` with `a < b`, or this raises."""
    for rec_a_key, rec_b_key in pairs:
        if not rec_a_key < rec_b_key:
            raise ValueError(
                f"{name} holds a non-canonical pair ({rec_a_key!r}, {rec_b_key!r}); "
                f"S5.0 requires rec_a_key < rec_b_key"
            )


def pairwise_metrics(
    predicted: set[Pair],
    truth: set[Pair],
    universe: set[Pair],
) -> PairwiseMetrics:
    """All three arguments are sets of canonical (rec_a_key, rec_b_key) pairs
    with rec_a_key < rec_b_key. Returns precision, recall, f1, and the raw
    tp/fp/fn counts, evaluated strictly within `universe`; a predicted or truth
    pair outside `universe` raises.

    Conventions (each unit-tested): empty ``predicted`` yields precision ``1.0``;
    empty ``truth`` yields recall ``1.0``; ``f1`` is ``0.0`` when
    ``precision + recall == 0``.

    Args:
        predicted: the pairs the system asserts — a blocked pair set, an
            above-`auto_merge` edge set, or a co-clustered closure (S8.5's table).
        truth: the true persona pairs, restricted by the caller to the same
            universe the prediction was made over.
        universe: the pair population both are evaluated within.

    Returns:
        The metrics and their raw counts.

    Raises:
        ValueError: a pair of any argument is not canonical, or a ``predicted`` or
            ``truth`` pair lies outside ``universe`` (which also covers a universe
            smaller than what was predicted over).
    """
    _require_canonical("predicted", predicted)
    _require_canonical("truth", truth)
    _require_canonical("universe", universe)

    stray_predicted = predicted - universe
    if stray_predicted:
        raise ValueError(
            f"{len(stray_predicted)} predicted pair(s) lie outside the universe, e.g. "
            f"{sorted(stray_predicted)[0]}; S8.5 evaluates strictly within `universe`"
        )
    stray_truth = truth - universe
    if stray_truth:
        raise ValueError(
            f"{len(stray_truth)} truth pair(s) lie outside the universe, e.g. "
            f"{sorted(stray_truth)[0]}; restrict the truth to the universe the "
            f"prediction was made over (S8.5)"
        )

    tp = len(predicted & truth)
    fp = len(predicted - truth)
    fn = len(truth - predicted)
    precision = 1.0 if not predicted else tp / (tp + fp)
    recall = 1.0 if not truth else tp / (tp + fn)
    f1 = 0.0 if precision + recall == 0 else 2 * precision * recall / (precision + recall)
    return PairwiseMetrics(tp=tp, fp=fp, fn=fn, precision=precision, recall=recall, f1=f1)
