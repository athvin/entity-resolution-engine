"""T-INV-1: the suite's only standing invariant (S8.3, S4.5.2).

S8.3 calls this "the only guard against the two implementations drifting apart" — the
DuckDB label-propagation path of S4.5.2 and Splink's
`cluster_pairwise_predictions_at_threshold`. Every individual probability can agree
while the two partitions disagree, so nothing in the scoring layer can catch that. This
helper can, and it runs after **every** integration scenario rather than in a test of
its own, because the drift appears as a consequence of whatever the scenario did.

S8.3 states five clauses, and all five are asserted here:

1. `entity_membership` induces the same partition as the connected components of the
   current edge set — assertion-adjusted, `>= auto_merge`, at the run's `model_version`,
   minus `cut_edges`;
2. every `entity_membership.entity_id` names an entity whose `status` is `active`;
3. exactly one membership row per `(source_system, source_record_id)`;
4. every `match_scores` / `assertions` / `review_queue` pair satisfies
   `rec_a_key < rec_b_key`;
5. zero `__splink__%` relations in `lake`.

**Clauses 1–3 are vacuous until ER-074, and that is deliberate.** `entity_membership`
is first written by the reconcile stage, which does not exist yet, so on today's board
the relation is empty. The helper therefore *passes* on an empty membership rather than
skipping. A skip would disarm the suite's only standing invariant for the whole of M4
and would have to be remembered and removed by a later ticket; a vacuous pass arms
clauses 4 and 5 now — which do real work today, since `match_scores`, `assertions`,
`review_queue` and the Splink scratch schema all exist — and starts asserting 1–3 the
moment the first membership row is written, with no edit here.

**The partition is recomputed, never read back from the same place it is checked
against.** Clause 1 derives components from the edge set with
:func:`~er.entities.cluster.label_propagate`; comparing `entity_membership` against
something derived from `entity_membership` would pass against any implementation at all.

`cut_edges` exclusion is ER-076's to extend: this helper reads the edge set the current
board produces, and the cut arm becomes meaningful when cuts are written.
"""

from __future__ import annotations

import os
from collections.abc import Sequence
from datetime import datetime
from pathlib import Path
from typing import Final

import duckdb

from er.config.loader import load_config
from er.entities.cluster import Edge, adjust_edges_with_assertions, label_propagate
from er.entities.events import EVENT_COLUMNS, REPLAY_ORDER_COLUMNS, replay_membership
from er.lake.model import SCHEMA_QUALIFIER
from er.matching.api import leaked_splink_relations
from er.review.assertions import active_assertions, check_contradiction_1

__all__ = [
    "CANONICAL_PAIR_RELATIONS",
    "Partition",
    "assert_membership_equals_components",
    "assert_replay_reproduces_membership",
    "current_partition",
    "membership_partition",
]

#: A set-partition of `record_key`s: the shape both sides of clause 1 are reduced to.
#: Frozensets rather than a labelling because the two sides name components differently
#: — Splink's `cluster_id` is the component minimum, an `entity_id` is a ULID — and the
#: claim is about the grouping (S4.5.2).
Partition = frozenset[frozenset[str]]

ENTITY_MEMBERSHIP: Final = f"{SCHEMA_QUALIFIER}.entity_membership"
ENTITIES: Final = f"{SCHEMA_QUALIFIER}.entities"
MATCH_SCORES: Final = f"{SCHEMA_QUALIFIER}.match_scores"
ASSERTIONS: Final = f"{SCHEMA_QUALIFIER}.assertions"
REVIEW_QUEUE: Final = f"{SCHEMA_QUALIFIER}.review_queue"
CUT_EDGES: Final = f"{SCHEMA_QUALIFIER}.cut_edges"

#: Clause 4's subjects. `review_queue` carries a nullable pair — an `entity`-subject row
#: has no endpoints (S4.3.5) — so the predicate is applied only where both are present;
#: a NULL endpoint is not a canonicalisation failure, it is a different kind of row.
CANONICAL_PAIR_RELATIONS: Final[tuple[str, ...]] = (MATCH_SCORES, ASSERTIONS, REVIEW_QUEUE)

#: The cap `label_propagate` is bounded by when this helper recomputes components. Read
#: from the caller's config where one is supplied; this is the fallback for a finalizer
#: that has no config in hand, and it is S6's own default.
DEFAULT_MAX_ITERATIONS: Final = 50


def _relation_exists(connection: duckdb.DuckDBPyConnection, qualified: str) -> bool:
    """Whether ``qualified`` is readable on this connection.

    The finalizer runs after every integration test, including ones whose namespace
    holds only the `ddl.py`-owned relations and ones that deliberately dropped
    everything. A missing relation is not an invariant violation — it is a lake that
    has not reached the stage where the clause could mean anything.
    """
    try:
        connection.execute(f"SELECT 1 FROM {qualified} LIMIT 0")
    except duckdb.Error:
        return False
    return True


def membership_partition(connection: duckdb.DuckDBPyConnection) -> Partition:
    """The partition `entity_membership` induces, grouped by `entity_id`."""
    if not _relation_exists(connection, ENTITY_MEMBERSHIP):
        return frozenset()
    rows = connection.execute(f"SELECT entity_id, record_key FROM {ENTITY_MEMBERSHIP}").fetchall()
    grouped: dict[str, set[str]] = {}
    for entity_id, key in rows:
        grouped.setdefault(str(entity_id), set()).add(str(key))
    return frozenset(frozenset(members) for members in grouped.values())


def current_partition(
    connection: duckdb.DuckDBPyConnection,
    nodes: Sequence[str],
    edges: Sequence[tuple[str, str]],
    *,
    max_iterations: int = DEFAULT_MAX_ITERATIONS,
) -> Partition:
    """The connected components of ``edges`` over ``nodes``, as a set-partition.

    Derived with :func:`~er.entities.cluster.label_propagate` — the same primitive the
    incremental path uses — so clause 1 compares `entity_membership` against a
    recomputation rather than against itself.

    Args:
        connection: any connection; the propagation runs in its in-memory database and
            writes nothing (S4.0b, M17).
        nodes: every record that may hold membership. A node with no edge is its own
            component, which is what makes the two sides comparable at all.
        edges: the current edge set, as canonical pairs.
        max_iterations: `clustering.max_iterations` (S6).

    Returns:
        The partition, one frozenset per component.
    """
    if not nodes:
        return frozenset()
    result = label_propagate(connection, nodes, edges, max_iterations=max_iterations)
    grouped: dict[str, set[str]] = {}
    for key, label in result.labels.items():
        grouped.setdefault(label, set()).add(key)
    return frozenset(frozenset(members) for members in grouped.values())


def _assert_one_row_per_record(connection: duckdb.DuckDBPyConnection) -> None:
    """Clause 3: exactly one `entity_membership` row per `(source_system, source_record_id)`.

    D3 makes `entity_membership` current state, so a second row for one record does not
    mean "two entities" — it means the relation can no longer answer the question it
    exists to answer, and every consumer that joins on it silently doubles.
    """
    duplicates = connection.execute(
        f"SELECT source_system, source_record_id, count(*) AS rows FROM {ENTITY_MEMBERSHIP} "
        "GROUP BY source_system, source_record_id HAVING count(*) > 1 "
        "ORDER BY source_system, source_record_id"
    ).fetchall()
    assert not duplicates, (
        "T-INV-1 clause 3: entity_membership holds more than one row for "
        f"{len(duplicates)} record(s); it is current state, one row per record (D3):\n"
        + "\n".join(
            f"    {system}:{record} -> {count} rows" for system, record, count in duplicates
        )
    )


def _assert_membership_targets_active_entities(connection: duckdb.DuckDBPyConnection) -> None:
    """Clause 2: every membership row names an `active` entity.

    A membership row pointing at a `merged` or `retired` entity is the shape INV-PERM
    forbids (S4.5.3): `merged_into` is a redirect for external id resolution, never a
    way to resolve current membership.
    """
    if not _relation_exists(connection, ENTITIES):
        return
    orphaned = connection.execute(
        f"SELECT m.record_key, m.entity_id, e.status FROM {ENTITY_MEMBERSHIP} AS m "
        f"LEFT JOIN {ENTITIES} AS e ON e.entity_id = m.entity_id "
        "WHERE e.entity_id IS NULL OR e.status <> 'active' "
        "ORDER BY m.record_key"
    ).fetchall()
    assert not orphaned, (
        f"T-INV-1 clause 2: {len(orphaned)} membership row(s) do not name an active "
        "entity; a merged or retired entity may never hold members (S4.5.3):\n"
        + "\n".join(
            f"    {key} -> {entity} (status={status if status is not None else 'MISSING'})"
            for key, entity, status in orphaned
        )
    )


def _assert_pairs_are_canonical(connection: duckdb.DuckDBPyConnection) -> None:
    """Clause 4: `rec_a_key < rec_b_key` wherever a pair is stored.

    S5.0 canonicalises at write time through one helper so that "readers never perform a
    two-sided join". A single reversed pair makes every such reader wrong, and it is
    invisible until a join silently returns nothing.
    """
    for relation in CANONICAL_PAIR_RELATIONS:
        if not _relation_exists(connection, relation):
            continue
        offenders = connection.execute(
            f"SELECT rec_a_key, rec_b_key FROM {relation} "
            "WHERE rec_a_key IS NOT NULL AND rec_b_key IS NOT NULL "
            "AND NOT (rec_a_key < rec_b_key) ORDER BY rec_a_key, rec_b_key"
        ).fetchall()
        assert not offenders, (
            f"T-INV-1 clause 4: {relation} holds {len(offenders)} non-canonical pair(s); "
            "S5.0 requires rec_a_key < rec_b_key on every stored pair:\n"
            + "\n".join(f"    {left!r} | {right!r}" for left, right in offenders[:20])
        )


def _assert_no_splink_relations(connection: duckdb.DuckDBPyConnection) -> None:
    """Clause 5: nothing of Splink's reached the lake (M17)."""
    leaked = leaked_splink_relations(connection)
    assert leaked == (), (
        f"T-INV-1 clause 5: {len(leaked)} __splink__ relation(s) are in the lake; the "
        f"scratch schema is in the in-memory database and nothing may escape it: {list(leaked)}"
    )


def assert_membership_equals_components(
    connection: duckdb.DuckDBPyConnection,
    *,
    nodes: Sequence[str] | None = None,
    edges: Sequence[tuple[str, str]] | None = None,
    max_iterations: int = DEFAULT_MAX_ITERATIONS,
) -> None:
    """Assert all five T-INV-1 clauses against ``connection`` (S8.3).

    Args:
        connection: a connection with the lake attached.
        nodes: the node set clause 1 recomputes over. ``None`` means "derive it from
            `entity_membership`", which is what the autouse finalizer passes — it has no
            scenario context, and a membership row is itself evidence the record exists.
        edges: the current edge set as canonical pairs. ``None`` derives it from the
            active, at-or-above-`auto_merge` rows a scenario left behind.
        max_iterations: `clustering.max_iterations` (S6).

    Raises:
        AssertionError: any clause fails. The message names the clause and the offending
            rows, because the finalizer runs after every scenario and "T-INV-1 failed"
            without a subject is a day of bisecting.
    """
    if not _relation_exists(connection, ENTITY_MEMBERSHIP):
        # A namespace with no ddl.py-owned relations at all: nothing to assert about
        # membership, and clauses 4 and 5 are still checked below.
        _assert_pairs_are_canonical(connection)
        _assert_no_splink_relations(connection)
        return

    _assert_one_row_per_record(connection)
    _assert_membership_targets_active_entities(connection)
    _assert_pairs_are_canonical(connection)
    _assert_no_splink_relations(connection)

    membership = membership_partition(connection)
    if not membership:
        # Clause 1 is vacuous before ER-074 writes the first membership row. It passes
        # rather than skips: see this module's header.
        return

    # An unsatisfiable assertion set voids clause 1's PREMISE, and that is not a
    # weakening. S4.4.1 makes a `never` inside an always-closure a hard pre-clustering
    # failure: the reconcile refuses and writes nothing, deliberately leaving membership
    # as it was. The active `always` edges are still injected by S4.4's adjustment, so
    # the recomputation merges components the pipeline was forbidden to merge — and
    # there is no partition that satisfies the set, so "membership equals the
    # components" has no correct answer to be measured against. The remaining four
    # clauses are asserted above and still hold; only the comparison is skipped, and
    # only while the contradiction stands.
    if _relation_exists(connection, ASSERTIONS) and check_contradiction_1(
        active_assertions(connection)
    ):
        return

    if nodes is None:
        nodes = [
            str(row[0])
            for row in connection.execute(
                f"SELECT DISTINCT record_key FROM {ENTITY_MEMBERSHIP} ORDER BY record_key"
            ).fetchall()
        ]
    if edges is None:
        edges = _current_edge_pairs(connection, nodes)

    components = current_partition(connection, nodes, edges, max_iterations=max_iterations)
    assert membership == components, _partition_report(membership, components)


def _current_edge_pairs(
    connection: duckdb.DuckDBPyConnection, nodes: Sequence[str]
) -> list[tuple[str, str]]:
    """S8.3's current edge set: assertion-adjusted, `>= auto_merge`, minus live cuts.

    All three qualifiers are load-bearing and each fails in a different direction.

    * **`>= auto_merge`.** `match_scores` holds everything from `review_low` up
      (S4.3.4), and the clustering cut is `auto_merge` (S4.3, D13). Recomputing over
      the whole persisted band would connect the gray band and report every queued pair
      as membership drift.
    * **Assertion-adjusted (S4.4).** An `always` edge exists only in `assertions` — it
      is never persisted to `match_scores`, which is what keeps that relation's
      `model_version` NOT NULL. A recomputation that read scores alone would split every
      entity a steward's `always` merged, and a `never` the pipeline honoured would look
      like an entity that failed to merge.
    * **Minus live cuts.** A cut pair is excluded from clustering (S4.4.2), so including
      it here would merge two components the pipeline deliberately keeps apart.

    The config is read from `ER_CONFIG`, the document S7.1 supplies to every process in
    the Compose envelope, because the finalizer that calls this has no config in hand
    and the threshold may not be guessed — M26.
    """
    if not _relation_exists(connection, MATCH_SCORES):
        return []
    cfg = load_config(Path(os.environ["ER_CONFIG"]))
    member = set(nodes)

    scored = [
        Edge(
            rec_a_key=str(left),
            rec_b_key=str(right),
            match_probability=float(probability),
        )
        for left, right, probability in connection.execute(
            f"SELECT DISTINCT rec_a_key, rec_b_key, match_probability FROM {MATCH_SCORES} "
            "WHERE is_active AND match_probability >= ?",
            [cfg.thresholds.auto_merge],
        ).fetchall()
    ]
    assertions = active_assertions(connection) if _relation_exists(connection, ASSERTIONS) else []
    adjusted = adjust_edges_with_assertions(scored, assertions, nodes=sorted(member))

    excluded: set[tuple[str, str]] = set()
    if _relation_exists(connection, CUT_EDGES):
        excluded = {
            (str(left), str(right))
            for left, right in connection.execute(
                f"SELECT rec_a_key, rec_b_key FROM {CUT_EDGES} WHERE active"
            ).fetchall()
        }
    return [
        edge.pair
        for edge in adjusted
        if edge.pair not in excluded and edge.rec_a_key in member and edge.rec_b_key in member
    ]


def _partition_report(membership: Partition, components: Partition) -> str:
    """Clause 1's failure message: which components differ, in both directions."""
    only_membership = sorted(sorted(group) for group in membership - components)
    only_components = sorted(sorted(group) for group in components - membership)
    return "\n".join(
        [
            "T-INV-1 clause 1: entity_membership does not equal the connected components "
            "of the current edge set (S4.5.2, S8.3). This is the DuckDB and Splink "
            "clustering paths disagreeing, or reconcile having written a partition "
            "neither produced.",
            f"  in entity_membership but not in the components ({len(only_membership)}):",
            *(f"    {group}" for group in only_membership[:20]),
            f"  in the components but not in entity_membership ({len(only_components)}):",
            *(f"    {group}" for group in only_components[:20]),
        ]
    )


ENTITY_EVENTS: Final = f"{SCHEMA_QUALIFIER}.entity_events"


def assert_replay_reproduces_membership(
    connection: duckdb.DuckDBPyConnection,
    *,
    up_to: tuple[datetime, int] | None = None,
    expected: dict[str, str] | None = None,
) -> dict[str, str]:
    """Folding `entity_events` reproduces `entity_membership` exactly (D3, M3).

    This is the only executable statement of what current state MEANS relative to its
    log: `entity_membership` holds no history (S4.5.3), so if the fold and the table
    disagree, one of them is lying and nothing else in the system would notice.

    The comparison runs in BOTH directions on purpose. A one-directional check —
    "everything replay produced is in the table" — passes on a fold that silently
    dropped an event type, which is exactly the bug worth catching here.

    Args:
        connection: an attached lake connection.
        up_to: fold only events at or before this `(occurred_at, seq)`, for the
            point-in-time arm. Every event, when omitted.
        expected: compare against this mapping rather than against the live table.
            The point-in-time arm passes a time-travelled read here, since the live
            table is the state AFTER the run being replayed to.

    Returns:
        The folded `record_key -> entity_id`, so a caller can make further claims about
        it without folding twice.
    """
    # Column names come from the S5 TableSpec, never from literals here: two of the five
    # this fold reads (`occurred_at`, `seq`) are VOLATILE_COLUMNS members, and
    # `tests/unit/test_compare_helpers.py` requires every helper to import that
    # vocabulary rather than re-spell it.
    stamp, sequence = REPLAY_ORDER_COLUMNS
    rows = connection.execute(f"SELECT {', '.join(EVENT_COLUMNS)} FROM {ENTITY_EVENTS}").fetchall()
    events = [dict(zip(EVENT_COLUMNS, row, strict=True)) for row in rows]
    if up_to is not None:
        events = [event for event in events if (event[stamp], int(event[sequence])) <= up_to]
    replayed = replay_membership(events)

    if expected is None:
        expected = {
            str(record_key): str(entity_id)
            for record_key, entity_id in connection.execute(
                f"SELECT record_key, entity_id FROM {ENTITY_MEMBERSHIP}"
            ).fetchall()
        }

    if replayed == expected:
        return replayed

    only_replay = sorted(k for k in replayed if replayed[k] != expected.get(k))
    only_table = sorted(k for k in expected if expected[k] != replayed.get(k))
    raise AssertionError(
        "\n".join(
            [
                f"replaying {len(events)} event(s) does not reproduce entity_membership "
                "(S4.5.3, D3)",
                f"  records replay places differently ({len(only_replay)}):",
                *(
                    f"    {k}: replay={replayed.get(k)!r} table={expected.get(k)!r}"
                    for k in only_replay[:20]
                ),
                f"  records the table holds that replay does not ({len(only_table)}):",
                *(
                    f"    {k}: table={expected.get(k)!r} replay={replayed.get(k)!r}"
                    for k in only_table[:20]
                ),
            ]
        )
    )
