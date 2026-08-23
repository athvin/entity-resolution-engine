"""S4.5.2's label propagation over hand-built graphs, on a bare `duckdb.connect()`.

The loop is SQL, but it is SQL over `TEMP` relations in the in-memory database (S4.0b) —
so everything about the ALGORITHM is answerable with no lake, no Docker and no fixture,
which is where S8.1 wants it. What is left for the integration layer is the pair of claims
that are about a *lake*: that the partition matches a reference over `base_10`'s affected
subgraph, and that the loop leaves `lake.main` untouched.

The four claims below are each a distinct way the incremental partition goes wrong, and
none of them is visible in the others:

* **The label is the component minimum over the CLOSED neighbourhood.** An open
  neighbourhood — `min` over the neighbours alone — loses a record's own key, and the
  minimum of a component then wanders instead of settling.
* **Pointer jumping, not one hop per round.** A 1024-record chain settles in
  `ceil(log2 1024) + 1 == 11` rounds. Without the jump it needs a thousand, so the default
  `clustering.max_iterations` of 50 would fail the stage on a corpus that has nothing wrong
  with it. This is the test that tells the two failure modes apart.
* **An isolated node labels itself.** S4.5.3's "a record leaving all clusters becomes a
  singleton" reads the propagation's output; a record dropped for having no incident edge
  never reaches the reconciler, and its entity is never retired.
* **Order and orientation change nothing.** D1/D2 (S4.5.4) are claims about the partition,
  and the input this stage gets is a list whose order is a scan order DuckDB does not
  guarantee.

Keys are built through :func:`~er.entities.ids.record_key` rather than written as
`"unit:r0000"` literals, because D6's scalar identity has one implementation and the
lexicographic order the loop's `min` uses is a property of what that helper emits. The
generated keys are zero-padded so that lexicographic order IS ascending order — otherwise
`unit:r10` sorts below `unit:r9` and "the minimum of the component" stops being the first
record of the chain.
"""

from __future__ import annotations

import random
from collections.abc import Iterator, Sequence
from typing import Final

import duckdb
import pytest

from er.entities.cluster import (
    LABEL_PROP_ITERATIONS,
    MAX_ITERATION_BOUND,
    label_propagate,
)
from er.entities.ids import record_key
from er.errors import ErrorClass, ExitCode, NonConvergenceError, exit_code_for
from er.obs.counters import DECLARED_COUNTERS

#: `clustering.max_iterations` as S6 defaults it. A literal here and read from the
#: validated config in production (M26): what these tests exercise is the loop's response
#: to a cap, and pinning the number is how the pointer-jumping test can claim that the
#: default is a safety net rather than the expected count.
MAX_ITERATIONS: Final = 50

#: The source system every generated key carries. `record_key` bans `':'` in either
#: component (S5.0), and these keys are only ever compared with each other.
SOURCE: Final = "unit"

#: The chain AC1 is stated over. 1024 is a power of two, which is the tight case for
#: `ceil(log2 n) + 1`: the bound and the actual round count are equal there, so an
#: implementation that lost one doubling fails this and nothing else.
PATH_NODES: Final = 1024

#: The chain the non-convergence test uses. Small enough to reason about and long enough
#: that one round cannot settle it.
SHORT_PATH_NODES: Final = 8

#: The stage that owns `label_prop_iterations` in S4.5.6.
RECONCILE_STAGE: Final = "reconcile"


def key(index: int) -> str:
    """The `record_key` of the `index`-th generated record, zero-padded to sort."""
    return record_key(SOURCE, f"r{index:04d}")


def path_graph(node_count: int) -> tuple[list[str], list[tuple[str, str]]]:
    """A chain of `node_count` records: the worst case for propagation distance.

    Every other shape of the same size has a smaller diameter and settles sooner, which is
    why the bound is asserted here and not over a star or a clique.
    """
    nodes = [key(index) for index in range(node_count)]
    return nodes, [(nodes[index], nodes[index + 1]) for index in range(node_count - 1)]


def loop_relations(connection: duckdb.DuckDBPyConnection) -> list[str]:
    """Every relation on the connection whose name the loop could have made."""
    rows = connection.execute(
        "SELECT table_name FROM duckdb_tables() WHERE table_name LIKE 'er_label_prop%' "
        "ORDER BY table_name"
    ).fetchall()
    return [str(name) for (name,) in rows]


@pytest.fixture
def connection() -> Iterator[duckdb.DuckDBPyConnection]:
    """A bare in-memory DuckDB, plus the check that the loop left nothing behind.

    The teardown is the executable half of "only the returned labelling ever leaves the
    function": a `SELECT` against the labels relation must fail, because the relation is
    gone. It runs on the success and the failure path alike, which is what S4.5.2's
    "no snapshot committed" needs to be true of the raise as well as of the return.
    """
    open_connection = duckdb.connect()
    try:
        yield open_connection
        assert loop_relations(open_connection) == [], (
            "the loop left its TEMP relations on the connection; only the returned "
            "labelling may leave it (S4.0b)"
        )
        with pytest.raises(duckdb.Error):
            open_connection.execute("SELECT * FROM er_label_prop_labels")
    finally:
        open_connection.close()


def propagate(
    connection: duckdb.DuckDBPyConnection,
    nodes: Sequence[str],
    edges: Sequence[tuple[str, str]],
    *,
    max_iterations: int = MAX_ITERATIONS,
) -> tuple[dict[str, str], int]:
    """One propagation, as `(labels, iterations)`."""
    result = label_propagate(connection, nodes, edges, max_iterations=max_iterations)
    return dict(result.labels), result.iterations


def test_min_label_over_closed_neighbourhood(connection: duckdb.DuckDBPyConnection) -> None:
    """Every record carries the minimum `record_key` of its own component, and no other.

    S4.5.2 states the rule as `label(v) = min(record_key)` over the closed neighbourhood
    of `v`, propagated to fixpoint — so the fixpoint is one label per component and that
    label is the component's smallest key. Two components, deliberately: a single one
    cannot distinguish "the minimum of the component" from "the minimum of the input", and
    the second component's minimum is not the first's.

    The chain is delivered with its middle edge first so that no assertion here can be
    satisfied by a single left-to-right sweep of the edge list.
    """
    a, b, c, d, e = (key(index) for index in range(5))
    labels, iterations = propagate(connection, [a, b, c, d, e], [(b, c), (a, b), (d, e)])

    assert labels == {a: a, b: a, c: a, d: d, e: d}
    assert set(labels.values()) == {a, d}, "one label per component, and it is its minimum"
    assert iterations >= 1


def test_pointer_jumping_converges_within_log2_bound(
    connection: duckdb.DuckDBPyConnection,
) -> None:
    """AC1: a 1024-record chain settles in `ceil(log2 1024) + 1 == 11` rounds.

    The claim that separates pointer jumping from plain propagation. The closed
    neighbourhood alone moves a label one hop per round, so this chain would need 1023 of
    them and would fail the stage at the S6 default of 50 — with `non_convergence` on a
    corpus that has nothing wrong with it. Composing the round with itself doubles the
    reach instead, and 1024 being a power of two makes the bound tight: the actual count
    equals it, so an implementation that lost one doubling has nowhere to hide.

    The counter payload is asserted here because the round count is what S4.5.6 declares
    (`label_prop_iterations`) and this is the test that knows what it should be.
    """
    nodes, edges = path_graph(PATH_NODES)
    result = label_propagate(connection, nodes, edges, max_iterations=MAX_ITERATIONS)

    assert MAX_ITERATION_BOUND(PATH_NODES) == 11
    assert result.iterations <= MAX_ITERATION_BOUND(PATH_NODES)
    assert set(result.labels) == set(nodes), "every node passed in is labelled"
    assert set(result.labels.values()) == {nodes[0]}, (
        "the chain is one component, so every record carries its minimum record_key"
    )
    assert result.iterations < MAX_ITERATIONS, (
        "clustering.max_iterations is the safety net, not the expected count (S4.5.2)"
    )
    assert result.counters == {LABEL_PROP_ITERATIONS: result.iterations}
    assert LABEL_PROP_ITERATIONS in DECLARED_COUNTERS[RECONCILE_STAGE], (
        "the counter this returns must be the name S4.5.6 declares for reconcile"
    )


def test_isolated_node_labels_itself(connection: duckdb.DuckDBPyConnection) -> None:
    """AC2: a node in `nodes` with no incident edge is returned, labelled with itself.

    The arm an implementation loses by deriving the node set from the edges. S4.5.3 gives
    "a record leaving all clusters becomes a singleton entity" as a normal outcome of
    reconciliation, and its input is exactly this row: the record is in `affected_nodes`
    — its entity was widened in, or its own edges were invalidated (S4.5.5) — and it is
    incident to nothing. Dropped here, it never reaches the reconciler, its old entity is
    never retired, and the loss is silent.
    """
    alone, left, right = (key(index) for index in range(3))
    labels, _ = propagate(connection, [alone, left, right], [(left, right)])

    assert labels[alone] == alone
    assert labels == {alone: alone, left: left, right: left}


def test_output_is_order_and_orientation_independent(
    connection: duckdb.DuckDBPyConnection,
) -> None:
    """AC3: shuffling the edges and swapping every pair's endpoints changes nothing.

    D1 and D2 (S4.5.4) are stated over the partition, and the propagation is what produces
    it. Neither input this function gets is ordered: the edge list comes back from a scan
    whose order DuckDB does not guarantee, and a pair's orientation is canonical in
    `match_scores` (S5.0) but not in an assertion-injected edge or in a caller's list. If
    either could move a label, the partition would be a property of the plan.

    Compared with `==` on the mappings, which is exact equality of both key sets and every
    label — not a partition comparison, which would pass even if a component's label had
    moved to a different member.
    """
    nodes, edges = path_graph(64)
    extra = [(key(70), key(71)), (key(71), key(72))]
    nodes = [*nodes, key(70), key(71), key(72), key(80)]
    edges = [*edges, *extra]

    labels, iterations = propagate(connection, nodes, edges)

    shuffler = random.Random(20260820)
    shuffled_nodes = list(nodes)
    shuffler.shuffle(shuffled_nodes)
    swapped = [(rec_b_key, rec_a_key) for rec_a_key, rec_b_key in edges]
    shuffler.shuffle(swapped)
    assert swapped != edges, "the shuffle changed nothing, so this test asserts nothing"

    with duckdb.connect() as second:
        again, again_iterations = propagate(second, shuffled_nodes, swapped)

    assert again == labels
    assert again_iterations == iterations, (
        "the round count moved with the input order; the fixpoint would then be a "
        "property of the scan order and not of the graph"
    )
    assert labels[key(80)] == key(80), "the isolated node survived the reordering"


def test_exceeding_max_iterations_raises_non_convergence(
    connection: duckdb.DuckDBPyConnection,
) -> None:
    """AC4 and AC5: the cap is a hard failure, classified `non_convergence`, exit ``1``.

    S4.5.2 leaves no third option — no partial partition, no best-effort labelling: the
    stage fails, no snapshot is committed and no event is emitted. `max_iterations=1` is
    the cheapest way to reach it, and an 8-record chain needs more than one round by the
    bound this module's other test pins.

    The message must name the component an operator has to investigate, and the size it
    names is the WHOLE chain — eight — not the partial label group the loop happened to
    have formed when it gave up. A partial group would report three records for a
    component of eight and send the investigation to the wrong place.

    The exit status is asserted on the error object rather than through a CLI invocation:
    S4.7 derives the code from the class, so a class carrying the wrong code is a defect
    of the taxonomy row and not of any command.
    """
    nodes, edges = path_graph(SHORT_PATH_NODES)

    with pytest.raises(NonConvergenceError) as raised:
        label_propagate(connection, nodes, edges, max_iterations=1)

    error = raised.value
    assert error.error_class == ErrorClass.NON_CONVERGENCE
    assert error.error_class == "non_convergence"
    assert error.code == int(ExitCode.STAGE_FAILURE) == 1
    assert exit_code_for(error) == int(ExitCode.STAGE_FAILURE)
    assert not error.retryable, "S4.7: investigate the logged component, do not re-run"

    message = str(error)
    assert f"holds {SHORT_PATH_NODES} record(s)" in message, (
        f"the message must carry the unconverged component's size (S4.5.2): {message}"
    )
    assert f"minimum record_key is {nodes[0]!r}" in message, (
        f"the message must carry the component's minimum record_key (S4.5.2): {message}"
    )
    assert "clustering.max_iterations=1" in message, (
        "the operator has to be told which cap was exhausted (S6, M26)"
    )
