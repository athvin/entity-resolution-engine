"""D5's cut search, as a pure function (S4.4.2).

Every claim S4.4.2 makes about *which* edge is cut is a claim about a total order, and
a total order is exactly the kind of thing that looks right until two candidates tie.
So each tiebreak gets a case built so the two answers differ — a graph where the
correct edge and the plausible-but-wrong one are distinguishable — rather than one
where any implementation would agree.

The three orders under test:

* **path**: `hop_count ASC`, then the lexicographically smallest vertex sequence;
* **edge**: `match_probability ASC`, then `rec_a_key ASC`, then `rec_b_key ASC`;
* **protection**: an edge at or above `cut_protect_probability` is never cut, and a
  path made entirely of protected edges escalates instead.

Why the escalation is exercised at `cut_protect_probability = auto_merge` rather than
at the `1.0` default: S4.4.2 says so itself. At `1.0` a fully protected path is a path
of `always`-assertion edges, which is CONTRADICTION-1 and has already failed the run
before clustering — the branch is a narrow residual reserved for a raised threshold.
Testing it at the default would mean building a graph the pipeline cannot reach.

These run on no connection at all. `never_cut_fixpoint` is pure, which is what makes
S4.7's "no `cut_edges` row, no `edge_cut` event, no membership write" on a
non-convergent run a property of the call graph rather than of a rollback.
"""

from __future__ import annotations

from datetime import UTC, datetime
from typing import Final

import pytest

from er.errors import ErrorClass, ExitCode, NonConvergenceError
from er.review.assertions import ALWAYS, NEVER, Assertion
from er.review.never_cut import (
    PathEdge,
    choose_cut_edge,
    never_cut_fixpoint,
    shortest_path,
)

#: S6's default: exactly the assertion-sourced edges S4.4 injects at `p = 1.0`.
PROTECT_DEFAULT: Final = 1.0

#: The strict reading S4.4.2 names, in which no ordinary edge is cuttable.
PROTECT_STRICT: Final = 0.95

MAX_ITERATIONS: Final = 50

STAMP: Final = datetime(2026, 1, 1, tzinfo=UTC).replace(tzinfo=None)


def assertion(left: str, right: str, kind: str = NEVER, *, active: bool = True) -> Assertion:
    """One assertion, with the fields the cut search reads and defaults elsewhere."""
    return Assertion(
        assertion_id=f"A-{kind}-{left}-{right}",
        rec_a_key=left,
        rec_b_key=right,
        kind=kind,
        active=active,
        created_by="test",
        created_at=STAMP,
    )


def test_cuts_minimum_probability_edge_on_shortest_path() -> None:
    """AC1: on a–b–c with never(a,c), the lower-probability hop is the one cut.

    The two hops differ only in probability, so an implementation that cut the first
    hop it walked — or the last — would pass a same-probability graph and fail here.
    """
    edges: list[PathEdge] = [("a", "b", 0.97), ("b", "c", 0.96)]
    result = never_cut_fixpoint(
        edges,
        [assertion("a", "c")],
        nodes=("a", "b", "c"),
        cut_protect_probability=PROTECT_DEFAULT,
        max_iterations=MAX_ITERATIONS,
    )

    assert result.edges_cut == 1, f"expected one cut, got {result.cuts}"
    cut = result.cuts[0]
    assert cut.pair == ("b", "c"), (
        f"cut {cut.pair} at p={cut.match_probability}; S4.4.2 cuts the MINIMUM-"
        "probability edge on the path, which is (b, c) at 0.96"
    )
    assert cut.match_probability == 0.96
    assert not result.escalations

    # And the cut actually separates them: a and c are no longer connected.
    remaining = [edge for edge in edges if (edge[0], edge[1]) not in result.cut_pairs]
    assert shortest_path("a", "c", remaining) is None, (
        "a and c are still connected after the cut; the point of a partition-level "
        "never is that no path survives"
    )


def test_cut_choice_tie_resolved_by_pair_keys() -> None:
    """AC2: equal probabilities break on `(rec_a_key ASC, rec_b_key ASC)`.

    Both hops sit at 0.96, so probability decides nothing and the pair keys must. The
    path is a–b–c, so the candidates are ("a","b") and ("b","c"); ("a","b") sorts first
    and is the edge that must be cut.
    """
    edges: list[PathEdge] = [("a", "b", 0.96), ("b", "c", 0.96)]
    result = never_cut_fixpoint(
        edges,
        [assertion("a", "c")],
        nodes=("a", "b", "c"),
        cut_protect_probability=PROTECT_DEFAULT,
        max_iterations=MAX_ITERATIONS,
    )

    assert result.edges_cut == 1
    assert result.cuts[0].pair == ("a", "b"), (
        f"cut {result.cuts[0].pair}; with both hops at 0.96 the tiebreak is "
        "(rec_a_key ASC, rec_b_key ASC), which selects ('a', 'b') and not ('b', 'c')"
    )


def test_path_tie_resolved_by_lexically_smallest_vertex_sequence() -> None:
    """AC3: two paths of equal hop count resolve to the smaller vertex sequence.

    a–m–z and a–n–z are both two hops. S4.4.2 picks a–m–z, so the cut must land on one
    of ITS hops. The two paths carry different probabilities so that choosing the wrong
    path cuts a different edge and the test can tell.
    """
    edges: list[PathEdge] = [
        ("a", "m", 0.99),
        ("m", "z", 0.98),
        ("a", "n", 0.60),
        ("n", "z", 0.61),
    ]
    path = shortest_path("a", "z", edges)
    assert path == ("a", "m", "z"), (
        f"shortest_path chose {path}; a–m–z and a–n–z are both two hops and "
        "('a','m','z') is the lexicographically smaller sequence (S4.4.2)"
    )

    result = never_cut_fixpoint(
        edges,
        [assertion("a", "z")],
        nodes=("a", "m", "n", "z"),
        cut_protect_probability=PROTECT_DEFAULT,
        max_iterations=MAX_ITERATIONS,
    )
    # The first round cuts on a–m–z; a and z stay connected through n, so the loop
    # continues and cuts again. Both cuts are asserted, because stopping after one
    # would leave the never pair co-clustered.
    assert result.edges_cut == 2, f"expected two rounds of cutting, got {result.cuts}"
    assert result.cuts[0].pair == ("m", "z"), (
        f"the first cut is {result.cuts[0].pair}; on a–m–z the minimum-probability hop "
        "is (m, z) at 0.98"
    )
    remaining = [edge for edge in edges if (edge[0], edge[1]) not in result.cut_pairs]
    assert shortest_path("a", "z", remaining) is None


def test_fully_protected_path_escalates_instead_of_cutting() -> None:
    """AC4: every path protected -> a `never_unsatisfiable` escalation, and no cut.

    Run at `cut_protect_probability = auto_merge`, which is S4.4.2's strict reading and
    the configuration in which the branch is reachable at all. The same graph at the
    `1.0` default must cut instead, and that half is asserted below — the two
    configurations are the point.
    """
    edges: list[PathEdge] = [("a", "b", 0.97), ("b", "c", 0.96)]
    never = [assertion("a", "c")]

    strict = never_cut_fixpoint(
        edges,
        never,
        nodes=("a", "b", "c"),
        cut_protect_probability=PROTECT_STRICT,
        max_iterations=MAX_ITERATIONS,
    )
    assert strict.edges_cut == 0, (
        f"cut {strict.cuts} with every path edge at or above {PROTECT_STRICT}; a "
        "protected edge is never cut (S4.4.2 step 3)"
    )
    assert len(strict.escalations) == 1, f"expected one escalation, got {strict.escalations}"
    left, right, assertion_id = strict.escalations[0]
    assert (left, right) == ("a", "c")
    assert assertion_id == never[0].assertion_id

    # The default protects only assertion-sourced edges, so the same graph is cuttable.
    default = never_cut_fixpoint(
        edges,
        never,
        nodes=("a", "b", "c"),
        cut_protect_probability=PROTECT_DEFAULT,
        max_iterations=MAX_ITERATIONS,
    )
    assert default.edges_cut == 1 and not default.escalations, (
        "at the 1.0 default the same graph must be cut, not escalated; both "
        "configurations are required to work (S4.4.2)"
    )


def test_exceeding_max_iterations_raises_non_convergence() -> None:
    """AC5: the bound is enforced, classified `non_convergence`, and writes nothing.

    The graph needs two rounds for ONE pair, which is the shape that actually stresses
    the bound: a and z are joined by two disjoint two-hop paths, so cutting on the first
    leaves them connected through the second. Two *independent* pairs would not do —
    the loop cuts every violation it finds in a round, so they settle together in one.

    The failure carries S4.7's class rather than a generic error, because "investigate
    the logged component" is the operator action and the class is what routes them
    there. Nothing is written on the way out: the fixpoint is pure, so the "no
    `cut_edges` row, no `edge_cut` event, no membership write" guarantee holds by
    construction.
    """
    edges: list[PathEdge] = [
        ("a", "m", 0.99),
        ("m", "z", 0.98),
        ("a", "n", 0.60),
        ("n", "z", 0.61),
    ]
    never = [assertion("a", "z")]

    with pytest.raises(NonConvergenceError) as refusal:
        never_cut_fixpoint(
            edges,
            never,
            nodes=("a", "m", "n", "z"),
            cut_protect_probability=PROTECT_DEFAULT,
            max_iterations=1,
        )

    assert refusal.value.error_class == ErrorClass.NON_CONVERGENCE, (
        f"classified {refusal.value.error_class!r}; S4.7 gives this loop the `non_convergence` row"
    )
    assert refusal.value.code == int(ExitCode.STAGE_FAILURE)
    message = str(refusal.value)
    assert "1 round" in message, f"the message does not name the bound: {message}"
    assert "a" in message and "z" in message, (
        f"the message does not name the surviving pair: {message}"
    )

    # The same graph settles when the bound allows the rounds it needs, which is what
    # makes the failure above a statement about the cap and not about the graph.
    settled = never_cut_fixpoint(
        edges,
        never,
        nodes=("a", "m", "n", "z"),
        cut_protect_probability=PROTECT_DEFAULT,
        max_iterations=MAX_ITERATIONS,
    )
    assert settled.edges_cut == 2 and settled.iterations == 2, (
        f"expected two rounds and two cuts, got {settled.iterations} round(s) and {settled.cuts}"
    )


def test_two_two_split_induced_by_cut() -> None:
    """A cut that splits one four-member component into two pairs.

    The case S8.4 names for the reconciler, reached from this side: the cut is what
    makes the split happen, and the fragments are what ER-073's fragment ordering then
    has to rank. Asserted here as components rather than entities, because this module
    knows nothing about entity ids.
    """
    edges: list[PathEdge] = [
        ("w", "x", 0.99),
        ("x", "y", 0.96),
        ("y", "z", 0.99),
    ]
    result = never_cut_fixpoint(
        edges,
        [assertion("w", "z")],
        nodes=("w", "x", "y", "z"),
        cut_protect_probability=PROTECT_DEFAULT,
        max_iterations=MAX_ITERATIONS,
    )

    assert result.edges_cut == 1
    assert result.cuts[0].pair == ("x", "y"), (
        f"cut {result.cuts[0].pair}; the middle hop at 0.96 is the minimum-probability "
        "edge and cutting it is what yields two pairs rather than a 3-1 split"
    )
    remaining = [edge for edge in edges if (edge[0], edge[1]) not in result.cut_pairs]
    assert shortest_path("w", "z", remaining) is None
    assert shortest_path("w", "x", remaining) == ("w", "x")
    assert shortest_path("y", "z", remaining) == ("y", "z")


def test_always_assertions_are_not_cut_candidates() -> None:
    """Only active `never` rows drive the search (S4.4.2 step 1).

    An `always` in the set must not produce a cut, and a retracted `never` must not
    either — S4.4 keeps retracted rows precisely so the difference is computable, and a
    search that read them would keep honouring a decision a steward has withdrawn.
    """
    edges: list[PathEdge] = [("a", "b", 0.97), ("b", "c", 0.96)]
    quiet = never_cut_fixpoint(
        edges,
        [assertion("a", "c", ALWAYS), assertion("a", "b", NEVER, active=False)],
        nodes=("a", "b", "c"),
        cut_protect_probability=PROTECT_DEFAULT,
        max_iterations=MAX_ITERATIONS,
    )
    assert quiet.edges_cut == 0 and not quiet.escalations, (
        f"an always assertion or a retracted never produced {quiet}; neither is a cut "
        "candidate (S4.4, S4.4.2)"
    )


def test_choose_cut_edge_returns_none_when_every_hop_is_protected() -> None:
    """The escalation predicate itself, isolated from the loop that acts on it."""
    edges: list[PathEdge] = [("a", "b", 1.0), ("b", "c", 1.0)]
    assert choose_cut_edge(("a", "b", "c"), edges, cut_protect_probability=PROTECT_DEFAULT) is None
    assert choose_cut_edge(("a", "b", "c"), edges, cut_protect_probability=1.01) == ("a", "b", 1.0)
