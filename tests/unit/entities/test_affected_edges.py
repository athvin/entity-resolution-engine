"""S4.4's edge adjustment as the pure function it is, over hand-built lists and no lake.

The loader half of ER-070 is a query and its claims are
`tests/integration/test_affected_edges.py`'s: cumulative `match_scores`, `cut_edges`,
`int_std_records` are three relations with three writers, and none of them is expressible
in a synthetic list. What lives here is everything the adjustment decides *after* the
rows are loaded, and two of those claims are only cheaply assertable at this layer:

* **The always/never conflict.** S4.4 fixes the order as `always` first, `never` second
  so the two precedence readings cannot disagree — and it also rejects that pair at write
  time (exit ``1``), so a lake cannot hold one. Constructing the assertion objects
  directly is the only way to exercise the ordering the spec bothered to fix.
* **The canonical-pair guard.** :class:`~er.entities.cluster.Edge` refuses a pair that is
  not `rec_a_key < rec_b_key` (S5.0). Reaching that against a real lake would mean
  writing a row that bypassed `canonicalize_pair`, which is the thing no code path is
  allowed to do.

The `nodes` argument is exercised here too. It confines INJECTION to the affected set,
and the failure it prevents — an active `always` between two records this run never
touched dragging its two endpoints into the subgraph — is a claim about which records
were passed in, not about what any relation holds.
"""

from __future__ import annotations

from datetime import datetime
from typing import Final

import duckdb
import pytest

from er.entities.cluster import (
    ASSERTION_EDGE_PROBABILITY,
    ASSERTION_EVIDENCE_SOURCE,
    Edge,
    adjust_edges_with_assertions,
    affected_edges,
)
from er.review.assertions import ALWAYS, NEVER, Assertion

#: `thresholds.auto_merge` for this module. Every probability below is placed relative
#: to it, and the injected edges are asserted against it: S4.4 puts an `always` at `1.0`,
#: which is above the clustering cut by construction (D13).
AUTO_MERGE: Final = 0.90

#: A scored probability comfortably above the cut, for the edges the adjustment must
#: leave exactly as it found them.
SCORED_PROBABILITY: Final = 0.97

#: The affected node set the injection is confined to. `crm:z` is deliberately NOT in it.
NODES: Final[frozenset[str]] = frozenset({"billing:b", "crm:a", "crm:d", "webforms:c"})

OUTSIDE: Final = "crm:z"

MODEL_VERSION: Final = "v0001"
TF_SNAPSHOT_ID: Final = "01JWMDTFSNAP00000000000070"

CREATED_AT: Final = datetime(2026, 3, 1, 0, 0, 0)


def assertion(rec_a_key: str, rec_b_key: str, kind: str, *, active: bool = True) -> Assertion:
    """One `assertions` row, canonical per S5.0, with an id derived from the pair.

    Derived rather than drawn from an :class:`~er.entities.ids.IdFactory` because the id
    is *read* here — it is what the injected `evidence` carries — and a counter would make
    the expected value depend on construction order.
    """
    assert rec_a_key < rec_b_key, f"({rec_a_key!r}, {rec_b_key!r}) is not canonical (S5.0)"
    return Assertion(
        assertion_id=f"{rec_a_key}|{rec_b_key}|{kind}",
        rec_a_key=rec_a_key,
        rec_b_key=rec_b_key,
        kind=kind,
        active=active,
        created_by="steward",
        created_at=CREATED_AT,
    )


def pairs(edges: list[Edge]) -> list[tuple[str, str]]:
    """The pairs of an adjusted edge list, in the order it came back in."""
    return [edge.pair for edge in edges]


def test_always_injection_and_evidence() -> None:
    """AC5: an active `always` injects exactly one edge at `p = 1.0`, carrying its id.

    The steward path. Nothing was ever scored for `(crm:a, crm:d)` — no blocking rule
    paired them — so without the injection a steward's merge would be durable (S4.4) and
    inert, and the entity they belong in would never form.

    The scored edge alongside it is the control: the adjustment adds, and does not
    rewrite. Its `evidence` stays ``None``, which is how a consumer tells a pair that has
    a `match_scores` row from one that must never be given one.
    """
    scored = Edge("billing:b", "crm:a", SCORED_PROBABILITY)
    always = assertion("crm:a", "crm:d", ALWAYS)

    adjusted = adjust_edges_with_assertions([scored], [always], nodes=NODES)

    assert pairs(adjusted) == [("billing:b", "crm:a"), ("crm:a", "crm:d")]
    injected = adjusted[1]
    assert injected.match_probability == ASSERTION_EDGE_PROBABILITY == 1.0
    assert injected.evidence == {
        "source": ASSERTION_EVIDENCE_SOURCE,
        "assertion_id": always.assertion_id,
    }
    assert injected.is_assertion
    assert injected.match_probability >= AUTO_MERGE, (
        "an assertion edge sits above the clustering cut by construction (S4.4, D13); "
        "one below it would make a steward-authored merge a no-op"
    )

    assert adjusted[0] == scored and adjusted[0].evidence is None
    assert not adjusted[0].is_assertion

    # EXACTLY one edge: an injection that appended rather than keyed on the pair would
    # double the assertion's edge the moment the pair was also scored.
    also_scored = Edge("crm:a", "crm:d", SCORED_PROBABILITY)
    both = adjust_edges_with_assertions([scored, also_scored], [always], nodes=NODES)
    assert pairs(both) == [("billing:b", "crm:a"), ("crm:a", "crm:d")]
    assert both[1].is_assertion, "the assertion supersedes the scored probability (S4.4)"


def test_never_removes_edge_after_always() -> None:
    """AC7: a pair carrying both an `always` and a `never` is absent from the result.

    S4.4 applies `always` first and `never` second "so the two orderings cannot
    disagree", and it rejects the conflicting insert at write time — so this input cannot
    be produced through `er assert` and is constructed directly, which is the only way
    the fixed ordering is checkable at all.

    Asserted with the assertion list in both orders. The claim is about the *phases* of
    the adjustment, not about iteration order, and an implementation that applied each
    assertion as it came to it would pass one of these two and fail the other.
    """
    scored = Edge("crm:a", "crm:d", 0.99)
    always = assertion("crm:a", "crm:d", ALWAYS)
    never = assertion("crm:a", "crm:d", NEVER)

    for order in ([always, never], [never, always]):
        adjusted = adjust_edges_with_assertions([scored], order, nodes=NODES)
        assert adjusted == [], (
            f"`never` dominates `always` for the same pair (S4.4); with the assertions "
            f"supplied as {[row.kind for row in order]} the pair survived"
        )

    # And on its own, a `never` removes the edge regardless of how it scored.
    assert adjust_edges_with_assertions([scored], [never], nodes=NODES) == []

    # A `never` for a pair nothing scored removes nothing and injects nothing.
    assert adjust_edges_with_assertions([], [never], nodes=NODES) == []


def test_threshold_and_canonical_ordering() -> None:
    """AC8: every edge out is canonical and appears once, and thresholding is the loader's.

    Three claims that together make the returned list safe to join on:

    * a non-canonical or self-referential pair is refused at construction, because S5.0
      canonicalises at write time and a reader that sorted the pair into shape would
      hide the producer that skipped that step;
    * the result is one edge per pair in canonical order, whatever order the input
      arrived in — a duplicated pair collapses rather than being clustered twice;
    * the adjustment does not re-apply `auto_merge`. The bound is S4.5.1's query's, and
      applying it a second time here would put the clustering cut in two places, which is
      exactly how two of them end up disagreeing.
    """
    with pytest.raises(ValueError, match="not canonical"):
        Edge("crm:d", "crm:a", SCORED_PROBABILITY)
    with pytest.raises(ValueError):
        Edge("crm:a", "crm:a", SCORED_PROBABILITY)

    unordered = [
        Edge("crm:a", "crm:d", SCORED_PROBABILITY),
        Edge("billing:b", "crm:a", SCORED_PROBABILITY),
        Edge("crm:a", "crm:d", 0.91),
    ]

    adjusted = adjust_edges_with_assertions(unordered, [], nodes=NODES)

    assert pairs(adjusted) == [("billing:b", "crm:a"), ("crm:a", "crm:d")]
    assert all(edge.rec_a_key < edge.rec_b_key for edge in adjusted)
    assert len({edge.pair for edge in adjusted}) == len(adjusted)

    banded = Edge("billing:b", "webforms:c", AUTO_MERGE - 0.05)
    assert adjust_edges_with_assertions([banded], [], nodes=NODES) == [banded], (
        "the adjustment re-thresholded a loaded edge; `p >= auto_merge` is applied once, "
        "in the S4.5.1 query"
    )


def test_injection_is_confined_to_the_affected_nodes() -> None:
    """An `always` reaching outside the affected set injects nothing into it.

    An active `always` is a standing constraint: it stays active for every later run,
    long after the run that created it. On a run that touched neither endpoint, injecting
    its edge would add two records the node formula deliberately left out — and label
    propagation would then run over an endpoint whose entity membership was never loaded.

    ``nodes=None`` is the other half of the same claim: the partner rule adjusts with no
    node set, because the partner it is looking for is precisely the record on the far
    end of that edge.
    """
    outward = assertion("crm:a", OUTSIDE, ALWAYS)

    confined = adjust_edges_with_assertions([], [outward], nodes=NODES)
    assert confined == [], f"{OUTSIDE} is not in the affected set and was pulled in anyway"

    unconfined = adjust_edges_with_assertions([], [outward])
    assert pairs(unconfined) == [("crm:a", OUTSIDE)]
    assert unconfined[0].is_assertion

    # One endpoint inside is not enough: an edge is only usable when both ends are.
    assert adjust_edges_with_assertions([], [outward], nodes=frozenset({"crm:a"})) == []


def test_retracted_assertions_adjust_nothing() -> None:
    """Only `active` rows adjust: a retracted `always` injects nothing, a `never` cuts nothing.

    S4.4 keeps retracted rows so the assertion delta is computable (S4.5.1), which means
    the adjustment is handed rows that are no longer constraints whenever a caller passes
    the whole relation rather than `active_assertions`. Reading `active` here is what
    stops a retracted `never` cutting an edge forever.
    """
    scored = Edge("crm:a", "crm:d", 0.99)
    retracted_never = assertion("crm:a", "crm:d", NEVER, active=False)
    retracted_always = assertion("billing:b", "webforms:c", ALWAYS, active=False)

    adjusted = adjust_edges_with_assertions(
        [scored], [retracted_never, retracted_always], nodes=NODES
    )

    assert adjusted == [scored]


def test_empty_affected_set_reads_no_relation() -> None:
    """A run with nothing affected returns no edges without querying anything.

    S4.0 gives `er reconcile` exit ``10`` on an empty affected set (ER-074's to wire),
    and the connection here proves the claim rather than illustrating it: it is a bare
    in-memory database with no lake attached, so any statement this function issued would
    raise. The second half asserts exactly that, so the first half cannot pass because
    the query happened to succeed.
    """
    with duckdb.connect() as connection:
        assert (
            affected_edges(
                connection,
                [],
                model_version=MODEL_VERSION,
                tf_snapshot_id=TF_SNAPSHOT_ID,
                auto_merge=AUTO_MERGE,
            )
            == []
        )

        with pytest.raises(duckdb.Error):
            affected_edges(
                connection,
                NODES,
                model_version=MODEL_VERSION,
                tf_snapshot_id=TF_SNAPSHOT_ID,
                auto_merge=AUTO_MERGE,
            )
