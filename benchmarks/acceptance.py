"""Evaluate quality budgets without confusing percentage points with fractions."""

from dataclasses import dataclass
from math import isfinite

from er.eval.metrics import PairwiseMetrics

MAX_RECALL_LOSS = 0.0001  # 0.01 percentage points, not 1% or 0.01.
MIN_SPEEDUP = 0.20


@dataclass(frozen=True)
class Verdict:
    accepted: bool
    reasons: tuple[str, ...]
    recall_loss_pp: float
    time_reduction: float


def evaluate(
    baseline: PairwiseMetrics,
    candidate: PairwiseMetrics,
    *,
    baseline_seconds: float,
    candidate_seconds: float,
    records: int,
    repetitions: int,
    other_workload_regressed: bool = False,
) -> Verdict:
    """One held-out dataset/workload; all datasets must pass before promotion."""
    if any(not isfinite(t) or t <= 0 for t in (baseline_seconds, candidate_seconds)):
        raise ValueError("timings must be finite and positive")
    if baseline.tp + baseline.fn != candidate.tp + candidate.fn:
        raise ValueError("truth populations differ")
    reasons = []
    # Integer cross-products avoid round-off disguising a precision regression.
    base_pairs, candidate_pairs = baseline.tp + baseline.fp, candidate.tp + candidate.fp
    if (
        candidate.tp * base_pairs < baseline.tp * candidate_pairs
        if base_pairs
        else candidate.fp > 0
    ):
        reasons.append("cluster precision decreased")
    truth = baseline.tp + baseline.fn
    missed = baseline.tp - candidate.tp
    loss = missed / truth if truth else 0.0
    reduction = 1 - candidate_seconds / baseline_seconds
    if missed * 10_000 > truth:
        reasons.append("cluster recall loss exceeds 0.01 percentage points")
    if loss > 0 and candidate_seconds > baseline_seconds * (1 - MIN_SPEEDUP):
        reasons.append("recall budget requires at least 20% end-to-end time reduction")
    if reduction <= 0:
        reasons.append("target workload did not get faster")
    if records < 10_000_000 or repetitions < 3:
        reasons.append("requires three 10M confirmation measurements")
    if other_workload_regressed:
        reasons.append("another workload has a repeatable slowdown")
    return Verdict(not reasons, tuple(reasons), loss * 100, reduction)
