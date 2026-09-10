"""`er.eval.pairwise_metrics`: the S8.5 contract, hand-computed (S8.4, S8.5).

Every value asserted here is worked out by hand in the test body, never by calling
the function twice: this file is the external authority the single implementation
is checked against, and a test that derived its expectation from the code under
test would prove reflexivity.
"""

from __future__ import annotations

from itertools import combinations

import pytest

from er.eval.metrics import PairwiseMetrics, pairwise_metrics

#: Three records, canonically ordered, and their full C(3,2) universe.
A, B, C = "s:a", "s:b", "s:c"
UNIVERSE = {(A, B), (A, C), (B, C)}


def test_hand_computed_case() -> None:
    """AC1: tp=1, fp=1, fn=1 -> 0.5 across the board, by hand."""
    metrics = pairwise_metrics({(A, B), (A, C)}, {(A, B), (B, C)}, set(UNIVERSE))
    # (A,B) is the one agreement; (A,C) is asserted but false; (B,C) is true but
    # missed. precision = 1/2, recall = 1/2, f1 = 2*.5*.5/(.5+.5) = .5.
    assert metrics == PairwiseMetrics(tp=1, fp=1, fn=1, precision=0.5, recall=0.5, f1=0.5)


def test_degenerate_inputs() -> None:
    """AC2: the pinned conventions, each stated over a non-trivial other side."""
    nothing_predicted = pairwise_metrics(set(), {(A, B)}, set(UNIVERSE))
    assert nothing_predicted == PairwiseMetrics(tp=0, fp=0, fn=1, precision=1.0, recall=0.0, f1=0.0)

    nothing_true = pairwise_metrics({(A, B)}, set(), set(UNIVERSE))
    assert nothing_true == PairwiseMetrics(tp=0, fp=1, fn=0, precision=0.0, recall=1.0, f1=0.0)

    both_empty = pairwise_metrics(set(), set(), set())
    assert both_empty == PairwiseMetrics(tp=0, fp=0, fn=0, precision=1.0, recall=1.0, f1=1.0)


def test_partition_shapes() -> None:
    """AC2's partition arms: all-singletons predicts nothing; all-in-one predicts all.

    Stated over five records with a two-persona truth (a 3-group and a 2-group,
    3 + 1 = 4 true pairs of the C(5,2) = 10), so both directions have real counts.
    """
    keys = [f"s:{index}" for index in range(5)]
    universe = {(a, b) for a, b in combinations(keys, 2)}
    truth = {pair for pair in combinations(keys[:3], 2)} | {(keys[3], keys[4])}
    assert len(truth) == 4

    singletons = pairwise_metrics(set(), truth, universe)
    assert (singletons.tp, singletons.fp, singletons.fn) == (0, 0, 4)
    assert (singletons.precision, singletons.recall) == (1.0, 0.0)

    one_cluster = pairwise_metrics(set(universe), truth, universe)
    assert (one_cluster.tp, one_cluster.fp, one_cluster.fn) == (4, 6, 0)
    assert one_cluster.precision == 0.4
    assert one_cluster.recall == 1.0
    # f1 = 2 * 0.4 * 1.0 / 1.4, by hand.
    assert one_cluster.f1 == pytest.approx(0.8 / 1.4)


def test_out_of_universe_and_non_canonical_raise() -> None:
    """AC3: each violation raises individually, coerced never."""
    with pytest.raises(ValueError, match="predicted pair"):
        pairwise_metrics({(A, "s:z")}, set(), set(UNIVERSE))
    with pytest.raises(ValueError, match="truth pair"):
        pairwise_metrics(set(), {(A, "s:z")}, set(UNIVERSE))
    # A universe smaller than what was predicted over is the same stray-pair error:
    # the surplus prediction cannot lie inside it.
    with pytest.raises(ValueError, match="predicted pair"):
        pairwise_metrics(set(UNIVERSE), set(), {(A, B)})
    with pytest.raises(ValueError, match="non-canonical"):
        pairwise_metrics({(B, A)}, set(), set(UNIVERSE))
    with pytest.raises(ValueError, match="non-canonical"):
        pairwise_metrics(set(), {(B, A)}, set(UNIVERSE))
    with pytest.raises(ValueError, match="non-canonical"):
        pairwise_metrics(set(), set(), {(A, A)})


def test_cluster_closure_pairs_is_canonical_and_transitive() -> None:
    """The closure is every within-entity pair, canonical, and never crosses.

    Membership rows arrive deliberately unsorted so canonicality is the
    function's doing, not the input's; the duplicate-record guard is S4.5.3's
    one-row-per-record reading of `entity_membership`.
    """
    from er.eval.metrics import cluster_closure_pairs, membership_partition

    rows = [("s:c", "E1"), ("s:a", "E1"), ("s:b", "E1"), ("s:z", "E2"), ("s:y", "E2")]
    closure = cluster_closure_pairs(rows)
    # C(3,2) within E1 plus the E2 pair — worked by hand — and nothing across.
    assert closure == {("s:a", "s:b"), ("s:a", "s:c"), ("s:b", "s:c"), ("s:y", "s:z")}
    assert all(rec_a < rec_b for rec_a, rec_b in closure)

    partition = membership_partition(rows)
    assert partition == {
        "E1": frozenset({"s:a", "s:b", "s:c"}),
        "E2": frozenset({"s:y", "s:z"}),
    }

    with pytest.raises(ValueError, match="two entities"):
        membership_partition([("s:a", "E1"), ("s:a", "E2")])
    # The same assignment twice is a re-statement, not a conflict.
    assert cluster_closure_pairs([("s:a", "E1"), ("s:a", "E1")]) == set()
