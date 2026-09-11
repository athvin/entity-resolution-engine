"""The S10.5 benchmark quality block — reported beside speed, never gated (S8.5, M21).

Three families, each a call to the ONE precision/recall implementation,
`er.eval.pairwise_metrics`, so a benchmark number and a test number can never be produced
by two code paths (S9.1; `scripts/lint_metrics.py` enforces there is no second formula):

* **Blocking recall** — the blocked pair set vs true persona pairs, over the full C(n,2)
  of current records. The only number that falls when a blocking rule stops emitting a
  key, because edge-level recall is blind to it (a dropped pair leaves both its predicted
  and its universe).
* **Edge-level** — `match_scores` at or above `auto_merge` vs true pairs RESTRICTED to the
  blocked set, over that blocked set.
* **Cluster-level (the headline)** — the transitive closure of `entity_membership` vs true
  pairs, over the full C(n,2). One bad edge chaining two 4-record clusters is 1 false pair
  at the edge level and 16 at the cluster level, which is why this is the headline.

This module builds the three `(predicted, truth, universe)` triples from the lake and the
generator's `persona_id` ground truth, and does no arithmetic of its own — every number
comes back from `pairwise_metrics`.
"""

from __future__ import annotations

from collections.abc import Iterable
from itertools import combinations
from typing import Any

from er.eval.metrics import Pair, PairwiseMetrics, pairwise_metrics

__all__ = [
    "blocking_recall",
    "cluster_level_metrics",
    "edge_level_metrics",
    "quality_block",
    "truth_pairs_from_rows",
]


def _canonical(a: str, b: str) -> Pair:
    return (a, b) if a < b else (b, a)


def truth_pairs_from_rows(rows: Iterable[tuple[str, str]]) -> set[Pair]:
    """Canonical true pairs from `(persona_id, record_key)` rows — the generator's sidecar.

    Every pair of records sharing a `persona_id` is a true pair; the grouping is the whole
    of the ground truth (S10.1, M21). `truth.csv` never reaches the pipeline.
    """
    by_persona: dict[str, list[str]] = {}
    for persona_id, record_key in rows:
        by_persona.setdefault(persona_id, []).append(record_key)
    pairs: set[Pair] = set()
    for members in by_persona.values():
        for a, b in combinations(sorted(set(members)), 2):
            pairs.add((a, b))
    return pairs


def _current_records(connection: Any) -> list[str]:
    return [
        str(row[0])
        for row in connection.execute(
            "SELECT record_key FROM lake.main.int_std_records ORDER BY record_key"
        ).fetchall()
    ]


def _all_pairs(records: Iterable[str]) -> set[Pair]:
    return set(combinations(sorted(set(records)), 2))


def _blocked_pairs(connection: Any) -> set[Pair]:
    return {
        _canonical(str(a), str(b))
        for a, b in connection.execute(
            "SELECT DISTINCT a.record_key, b.record_key "
            "FROM lake.main.int_blocking_keys a JOIN lake.main.int_blocking_keys b "
            "ON a.key_type = b.key_type AND a.key_value = b.key_value "
            "AND a.record_key < b.record_key"
        ).fetchall()
    }


def _edge_pairs(connection: Any, auto_merge: float) -> set[Pair]:
    return {
        _canonical(str(a), str(b))
        for a, b in connection.execute(
            "SELECT rec_a_key, rec_b_key FROM lake.main.match_scores "
            "WHERE is_active AND match_probability >= ?",
            [auto_merge],
        ).fetchall()
    }


def _cluster_pairs(connection: Any) -> set[Pair]:
    members: dict[str, list[str]] = {}
    for record_key, entity_id in connection.execute(
        "SELECT record_key, entity_id FROM lake.main.entity_membership"
    ).fetchall():
        members.setdefault(str(entity_id), []).append(str(record_key))
    pairs: set[Pair] = set()
    for group in members.values():
        for a, b in combinations(sorted(set(group)), 2):
            pairs.add((a, b))
    return pairs


def blocking_recall(connection: Any, truth: set[Pair]) -> PairwiseMetrics:
    """S8.5 blocking family: blocked pairs vs truth over the full C(n,2) universe."""
    universe = _all_pairs(_current_records(connection))
    return pairwise_metrics(_blocked_pairs(connection), truth, universe)


def edge_level_metrics(connection: Any, truth: set[Pair], *, auto_merge: float) -> PairwiseMetrics:
    """S8.5 edge family: above-`auto_merge` edges vs truth, both over the blocked set."""
    blocked = _blocked_pairs(connection)
    predicted = _edge_pairs(connection, auto_merge) & blocked
    return pairwise_metrics(predicted, truth & blocked, blocked)


def cluster_level_metrics(connection: Any, truth: set[Pair]) -> PairwiseMetrics:
    """S8.5 cluster family (headline): membership closure vs truth over the full C(n,2)."""
    universe = _all_pairs(_current_records(connection))
    return pairwise_metrics(_cluster_pairs(connection), truth, universe)


def _family(metrics: PairwiseMetrics) -> dict[str, Any]:
    return {
        "precision": metrics.precision,
        "recall": metrics.recall,
        "f1": metrics.f1,
        "tp": metrics.tp,
        "fp": metrics.fp,
        "fn": metrics.fn,
    }


def quality_block(connection: Any, truth: set[Pair], *, auto_merge: float) -> dict[str, Any]:
    """The run document's quality contribution: the three families and `blocking_recall`.

    Returns a dict with ``blocking_recall`` (the top-level S8.5 required key) and
    ``quality`` (the schema's six-number block), plus the full per-family counts under
    ``families`` for the report table. Exactly three `pairwise_metrics` calls — one per
    family — and no arithmetic here.
    """
    blocking = blocking_recall(connection, truth)
    edge = edge_level_metrics(connection, truth, auto_merge=auto_merge)
    cluster = cluster_level_metrics(connection, truth)
    return {
        "blocking_recall": blocking.recall,
        "quality": {
            "edge_precision": edge.precision,
            "edge_recall": edge.recall,
            "edge_f1": edge.f1,
            "cluster_precision": cluster.precision,
            "cluster_recall": cluster.recall,
            "cluster_f1": cluster.f1,
        },
        "families": {
            "blocking": _family(blocking),
            "edge": _family(edge),
            "cluster": _family(cluster),
        },
    }
