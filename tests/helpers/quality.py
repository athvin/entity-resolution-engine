"""The three S8.5 metric triples, built once (DesignDoc.md S8.5, S8.2.1, S5.0).

`er.eval.pairwise_metrics` is the only place a precision or recall may be computed
(`scripts/lint_metrics.py` enforces it), so this module deliberately computes NO
metric: it builds the `predicted` / `truth` / `universe` sets the S8.5 table names,
each in the canonical `rec_a_key < rec_b_key` orientation, and hands them over.

Each builder delegates to the existing single reader of its source — the truth file
through :mod:`helpers.traps`, the blocked set through :mod:`helpers.pairs` — for the
reason both of those modules give: a second parser of the same committed file is a
place for the two to disagree.
"""

from __future__ import annotations

from itertools import combinations
from pathlib import Path

import duckdb

from er.entities.ids import canonicalize_pair
from er.lake.model import SCHEMA_QUALIFIER
from helpers.pairs import canonical_pairs_from_blocking_keys
from helpers.traps import Pair, persona_members, true_pairs_from_truth

__all__ = [
    "all_pairs_universe",
    "blocked_universe",
    "predicted_edges_at",
    "truth_pairs",
]

_MATCH_SCORES = f"{SCHEMA_QUALIFIER}.match_scores"


def truth_pairs(truth_csv: Path) -> set[Pair]:
    """The true persona pairs of a committed `truth.csv`, canonical (S8.2.1)."""
    return set(true_pairs_from_truth(truth_csv))


def all_pairs_universe(truth_csv: Path) -> set[Pair]:
    """The full `C(n, 2)` universe over the truth file's records (S8.5, blocking recall).

    Built from the committed truth rather than from `int_std_records` so the universe
    is a property of the fixture: a standardization defect that dropped a record would
    then shrink the universe and hide itself from the very number meant to catch it.
    """
    members = sorted(key for keys in persona_members(truth_csv).values() for key in keys)
    return {canonicalize_pair(a, b) for a, b in combinations(members, 2)}


def blocked_universe(connection: duckdb.DuckDBPyConnection) -> set[Pair]:
    """The DISTINCT canonical blocked pair set (S8.5's edge-level universe)."""
    return canonical_pairs_from_blocking_keys(connection)


def predicted_edges_at(connection: duckdb.DuckDBPyConnection, threshold: float) -> set[Pair]:
    """Active `match_scores` pairs at or above ``threshold``, canonical.

    Active only: an edge S4.5.5 invalidated is about a record version that no longer
    exists, and counting it would grade the scorer on evidence it has already retired.
    """
    rows = connection.execute(
        f"SELECT rec_a_key, rec_b_key FROM {_MATCH_SCORES} "
        f"WHERE is_active AND match_probability >= ?",
        [threshold],
    ).fetchall()
    return {canonicalize_pair(str(rec_a), str(rec_b)) for rec_a, rec_b in rows}
