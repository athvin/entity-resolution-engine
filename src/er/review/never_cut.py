"""D5: partition-level `never_match`, enforced after clustering (S4.4.2).

Deleting the edge `(a, b)` does not stop `a–c–b` re-linking the pair under connected
components, so a `never` assertion that only removed its own edge would be honoured
exactly until a third record bridged it. S4.4.2 therefore enforces `never` at the
**partition** level: after clustering, every active `never` pair whose endpoints are
co-clustered has the shortest path between them cut.

**Both orders are total, and that is the whole design.** A cut is a destructive,
persisted decision, so "which edge" may not depend on scan order:

* the **path** is chosen by ``(hop_count ASC, then the lexically smallest vertex
  sequence)``;
* the **edge on it** by ``(match_probability ASC, rec_a_key ASC, rec_b_key ASC)``.

Two runs over one graph therefore cut the same edge, which is what makes a cut
reproducible from `cut_edges` and what lets S4.5.4's determinism claim survive an
assertion.

**Protection and the escalation nobody reaches yet.** An edge at or above
`clustering.cut_protect_probability` is never cut. The default is ``1.0``, which
protects exactly the assertion-sourced edges S4.4 injects at ``p = 1.0``; setting it to
`auto_merge` yields the strict reading in which no ordinary edge is cuttable. When every
path between the endpoints is fully protected the pair is escalated to `review_queue`
with ``reason='never_unsatisfiable'`` rather than cut. S4.4.2 is candid that at the
default this branch is a **narrow residual** — a fully protected path is a path of
`always` edges, which is CONTRADICTION-1 and has already failed the run before
clustering — so it is specified, implemented and tested at
``cut_protect_probability = auto_merge``, the configuration where it is reachable.

**The fixpoint is bounded and its failure is hard.** Cutting is monotone: each round
removes at least one edge from a finite set. So exceeding `clustering.max_iterations`
does not mean slow convergence, it means a protected-edge cycle or a bug — the stage
fails with `non_convergence` (S4.7), and because nothing here writes, the "no
`cut_edges` row, no `edge_cut` event, no membership write" guarantee is a property of
the call graph rather than of a rollback.

**Nothing in the pure half touches a connection.** :func:`never_cut_fixpoint` takes the
edge set and returns what to cut; :func:`persist_cuts` and :func:`release_cuts` are the
only writers, and they are called by the reconcile stage inside the same apply step as
membership and events — so a reconcile that fails after the cut search leaves no cut
row behind.
"""

from __future__ import annotations

from collections import deque
from collections.abc import Iterable, Mapping, Sequence
from dataclasses import dataclass
from datetime import UTC, datetime
from typing import Final

import duckdb

from er.entities.ids import IdFactory, MonotonicUlidFactory, canonicalize_pair
from er.errors import NonConvergenceError
from er.lake.model import SCHEMA_QUALIFIER
from er.review.assertions import NEVER, Assertion

__all__ = [
    "CUT_EDGES_RELATION",
    "Cut",
    "CutResult",
    "PathEdge",
    "choose_cut_edge",
    "never_cut_fixpoint",
    "persist_cuts",
    "release_cuts",
    "shortest_path",
]

CUT_EDGES_RELATION: Final = "cut_edges"
_CUT_EDGES: Final = f"{SCHEMA_QUALIFIER}.{CUT_EDGES_RELATION}"

#: `cut_edges` in S5 DDL order. The run of a cut goes in `cut_run_id`, never `run_id` —
#: the relation has no `run_id` column, and writing one would be a column that does not
#: exist rather than the wrong value.
_CUT_COLUMNS: Final[tuple[str, ...]] = (
    "cut_id",
    "rec_a_key",
    "rec_b_key",
    "match_probability",
    "model_version",
    "tf_snapshot_id",
    "assertion_id",
    "active",
    "cut_run_id",
    "cut_at",
    "released_run_id",
    "released_at",
)

#: One edge of the clustering graph, as this module needs it: a canonical pair and the
#: probability the cut order sorts on.
PathEdge = tuple[str, str, float]


@dataclass(frozen=True)
class Cut:
    """One edge this run decided to cut, and the assertion that required it."""

    rec_a_key: str
    rec_b_key: str
    match_probability: float
    #: The `never` whose endpoints the cut separates. S5 records it so a released cut
    #: can be traced to the retraction that released it.
    assertion_id: str

    @property
    def pair(self) -> tuple[str, str]:
        return (self.rec_a_key, self.rec_b_key)


@dataclass(frozen=True)
class CutResult:
    """What the fixpoint decided: the cuts, the escalations, and the rounds it took."""

    cuts: tuple[Cut, ...] = ()
    #: `never` pairs every path between which was fully protected (S4.4.2 step 4).
    escalations: tuple[tuple[str, str, str], ...] = ()
    iterations: int = 0

    @property
    def edges_cut(self) -> int:
        return len(self.cuts)

    @property
    def cut_pairs(self) -> frozenset[tuple[str, str]]:
        """The pairs to remove from the edge set, for a caller recomputing components."""
        return frozenset(cut.pair for cut in self.cuts)


def _adjacency(edges: Iterable[PathEdge]) -> dict[str, list[str]]:
    """Undirected adjacency with every neighbour list sorted.

    Sorted because the lexicographic path tiebreak of S4.4.2 is only well defined if the
    search visits neighbours in a fixed order; an unsorted list would make the chosen
    path depend on the order the edge set arrived in.
    """
    graph: dict[str, set[str]] = {}
    for left, right, _ in edges:
        graph.setdefault(left, set()).add(right)
        graph.setdefault(right, set()).add(left)
    return {node: sorted(neighbours) for node, neighbours in sorted(graph.items())}


def shortest_path(source: str, target: str, edges: Iterable[PathEdge]) -> tuple[str, ...] | None:
    """The S4.4.2 path: fewest hops, then the lexicographically smallest vertex sequence.

    A breadth-first search settles `hop_count ASC` on its own. The second key is what
    makes the answer unique, and it is *not* free: BFS reaches a node by whichever
    predecessor it happened to expand first, so the path has to be compared rather than
    assumed. Each node therefore keeps the best sequence found at its own depth, and a
    later predecessor at the same depth replaces it only if its sequence sorts smaller.

    Args:
        source: one endpoint of the `never` pair.
        target: the other.
        edges: the clustering edge set.

    Returns:
        The vertex sequence from ``source`` to ``target``, or ``None`` when they are not
        connected — which is the ordinary case once a cut has separated them.
    """
    if source == target:
        return (source,)
    graph = _adjacency(edges)
    if source not in graph or target not in graph:
        return None

    best: dict[str, tuple[str, ...]] = {source: (source,)}
    depth: dict[str, int] = {source: 0}
    frontier: deque[str] = deque([source])
    while frontier:
        node = frontier.popleft()
        for neighbour in graph[node]:
            candidate = (*best[node], neighbour)
            if neighbour not in depth:
                depth[neighbour] = depth[node] + 1
                best[neighbour] = candidate
                frontier.append(neighbour)
            elif depth[neighbour] == depth[node] + 1 and candidate < best[neighbour]:
                # Same distance, smaller sequence: S4.4.2's second key. Rewriting the
                # entry is enough because every node further on is re-expanded from it
                # only while it sits in the frontier, and a node at this depth has not
                # been popped yet.
                best[neighbour] = candidate
    return best.get(target)


def choose_cut_edge(
    path: Sequence[str], edges: Iterable[PathEdge], *, cut_protect_probability: float
) -> PathEdge | None:
    """The minimum-probability unprotected edge on ``path`` (S4.4.2 step 2 and 3).

    Args:
        path: a vertex sequence from :func:`shortest_path`.
        edges: the clustering edge set, to read each hop's probability from.
        cut_protect_probability: `clustering.cut_protect_probability`. An edge at or
            above it is protected and is never cut.

    Returns:
        The edge to cut, or ``None`` when every hop on the path is protected — which is
        the escalation condition of step 4.
    """
    probability = {canonicalize_pair(left, right): value for left, right, value in edges}
    candidates: list[PathEdge] = []
    for left, right in zip(path, path[1:], strict=False):
        pair = canonicalize_pair(left, right)
        value = probability.get(pair)
        if value is None or value >= cut_protect_probability:
            continue
        candidates.append((pair[0], pair[1], value))
    if not candidates:
        return None
    # The cut order, in full: probability first, then the pair keys. Sorting on the
    # whole tuple is the total order S4.4.2 states, and it is what makes two runs over
    # one graph cut the same edge.
    return min(candidates, key=lambda edge: (edge[2], edge[0], edge[1]))


def _components(edges: Iterable[PathEdge], nodes: Iterable[str]) -> dict[str, int]:
    """Node -> component index, for deciding which `never` pairs are co-clustered."""
    graph = _adjacency(edges)
    seen: dict[str, int] = {}
    index = 0
    for node in sorted(set(nodes) | set(graph)):
        if node in seen:
            continue
        stack = [node]
        seen[node] = index
        while stack:
            current = stack.pop()
            for neighbour in graph.get(current, ()):
                if neighbour not in seen:
                    seen[neighbour] = index
                    stack.append(neighbour)
        index += 1
    return seen


def never_cut_fixpoint(
    edges: Iterable[PathEdge],
    assertions: Iterable[Assertion],
    *,
    nodes: Iterable[str] = (),
    cut_protect_probability: float,
    max_iterations: int,
) -> CutResult:
    """Cut until no active `never` pair is co-clustered, or fail (S4.4.2 step 6).

    PURE: no connection, no clock, no write. That is what makes the S4.7 guarantee —
    a non-convergent run commits no snapshot and emits no event — a property of where
    this function sits rather than of a rollback.

    Args:
        edges: the clustering edge set, post-assertion-adjustment.
        assertions: the assertion set; only active `never` rows are read.
        nodes: the node set, so an isolated endpoint is still a component.
        cut_protect_probability: `clustering.cut_protect_probability` (S6).
        max_iterations: `clustering.max_iterations` (S6).

    Returns:
        The cuts to persist, the pairs to escalate, and the rounds taken.

    Raises:
        er.errors.NonConvergenceError: violations remain after ``max_iterations``.
            Cutting is monotone, so this means a protected-edge cycle or a bug (S4.4.2).
    """
    live = list(edges)
    never = [assertion for assertion in assertions if assertion.kind == NEVER and assertion.active]
    if not never:
        return CutResult()

    cuts: list[Cut] = []
    escalated: list[tuple[str, str, str]] = []
    escalated_pairs: set[tuple[str, str]] = set()
    every_node = set(nodes)
    for left, right, _ in live:
        every_node |= {left, right}

    iterations = 0
    while True:
        component = _components(live, every_node)
        violations = [
            assertion
            for assertion in never
            if canonicalize_pair(assertion.rec_a_key, assertion.rec_b_key) not in escalated_pairs
            and component.get(assertion.rec_a_key) is not None
            and component.get(assertion.rec_a_key) == component.get(assertion.rec_b_key)
        ]
        if not violations:
            return CutResult(cuts=tuple(cuts), escalations=tuple(escalated), iterations=iterations)
        if iterations >= max_iterations:
            remaining = sorted(
                canonicalize_pair(assertion.rec_a_key, assertion.rec_b_key)
                for assertion in violations
            )
            raise NonConvergenceError(
                f"the S4.4.2 never-cut loop did not settle in {max_iterations} round(s); "
                f"{len(remaining)} pair(s) remain co-clustered: {remaining[:10]}. Cutting "
                "is monotone, so this is a protected-edge cycle or a defect, not slow "
                "convergence"
            )

        iterations += 1
        progressed = False
        for assertion in violations:
            path = shortest_path(assertion.rec_a_key, assertion.rec_b_key, live)
            if path is None:
                continue
            edge = choose_cut_edge(path, live, cut_protect_probability=cut_protect_probability)
            if edge is None:
                pair = canonicalize_pair(assertion.rec_a_key, assertion.rec_b_key)
                escalated.append((pair[0], pair[1], assertion.assertion_id))
                escalated_pairs.add(pair)
                progressed = True
                continue
            cuts.append(
                Cut(
                    rec_a_key=edge[0],
                    rec_b_key=edge[1],
                    match_probability=edge[2],
                    assertion_id=assertion.assertion_id,
                )
            )
            live = [
                candidate
                for candidate in live
                if canonicalize_pair(candidate[0], candidate[1]) != (edge[0], edge[1])
            ]
            progressed = True
        if not progressed:
            # Every remaining violation is unreachable and unescalatable, which the
            # loop above cannot produce; guarding it keeps the bound honest rather than
            # spinning to `max_iterations` for a reason the message would misname.
            raise NonConvergenceError(
                "the S4.4.2 never-cut loop made no progress on "
                f"{len(violations)} co-clustered never pair(s)"
            )


def persist_cuts(
    connection: duckdb.DuckDBPyConnection,
    cuts: Iterable[Cut],
    *,
    run_id: str,
    model_version: str,
    tf_snapshot_id: str,
    cut_at: datetime | None = None,
    cut_ids: Mapping[tuple[str, str], str] | None = None,
    ids: IdFactory | None = None,
) -> int:
    """Write ``cuts`` to `cut_edges`, one statement. Returns the rows written.

    A cut already recorded and still `active` for the same pair is not written again:
    S4.4.2 keeps a cut until its assertion is retracted or an endpoint's `content_hash`
    changes, so a second run over an unchanged corpus must add nothing. Without that,
    `cut_edges` would grow one row per run and the release path would have to guess
    which one to deactivate.
    """
    pending = list(cuts)
    if not pending:
        return 0
    stamp = datetime.now(UTC).replace(tzinfo=None) if cut_at is None else cut_at
    factory: IdFactory = MonotonicUlidFactory() if ids is None else ids
    minted: Mapping[tuple[str, str], str] = {} if cut_ids is None else cut_ids

    existing = {
        (str(left), str(right))
        for left, right in connection.execute(
            f"SELECT rec_a_key, rec_b_key FROM {_CUT_EDGES} WHERE active"
        ).fetchall()
    }
    fresh = [cut for cut in pending if cut.pair not in existing]
    if not fresh:
        return 0

    placeholders = ", ".join("(" + ", ".join("?" for _ in _CUT_COLUMNS) + ")" for _ in fresh)
    values: list[object] = []
    for cut in fresh:
        values += [
            minted.get(cut.pair) or factory.new(),
            cut.rec_a_key,
            cut.rec_b_key,
            cut.match_probability,
            model_version,
            tf_snapshot_id,
            cut.assertion_id,
            True,
            run_id,
            stamp,
            None,
            None,
        ]
    connection.execute(
        f"INSERT INTO {_CUT_EDGES} ({', '.join(_CUT_COLUMNS)}) VALUES {placeholders}",
        values,
    )
    return len(fresh)


def release_cuts(
    connection: duckdb.DuckDBPyConnection,
    *,
    run_id: str,
    released_at: datetime | None = None,
) -> int:
    """Deactivate every cut whose `never` is no longer active. Returns rows released.

    S4.4.2: "A cut is invalidated only when its assertion is retracted, or when either
    endpoint's `content_hash` changes." This is the first half — the retraction path —
    and it is a single `UPDATE` so the release lands in one snapshot beside the
    membership the next clustering produces. The `content_hash` half belongs to the
    invalidation tickets that own S4.5.5 and is not reached from here.
    """
    stamp = datetime.now(UTC).replace(tzinfo=None) if released_at is None else released_at
    assertions = f"{SCHEMA_QUALIFIER}.assertions"
    released = connection.execute(
        f"SELECT count(*) FROM {_CUT_EDGES} AS c WHERE c.active AND NOT EXISTS ("
        f"SELECT 1 FROM {assertions} AS a WHERE a.assertion_id = c.assertion_id AND a.active)"
    ).fetchone()
    count = 0 if released is None else int(released[0])
    if not count:
        return 0
    connection.execute(
        f"UPDATE {_CUT_EDGES} AS c SET active = false, released_run_id = ?, released_at = ? "
        f"WHERE c.active AND NOT EXISTS ("
        f"SELECT 1 FROM {assertions} AS a WHERE a.assertion_id = c.assertion_id AND a.active)",
        [run_id, stamp],
    )
    return count
