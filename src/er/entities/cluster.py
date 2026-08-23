"""The affected NODE and EDGE sets of S4.5.1 — the formulae, not a paraphrase of them.

S4.5.1 states the set as four lines of set algebra, and the whole reason it is a
formula is that every prose retelling of it has lost an arm:

```
seed  = records in this run's ingest batches
      ∪ records whose content_hash changed since the last successful run
      ∪ records tombstoned or resurrected since the last successful run
      ∪ records referenced by assertions created or retracted since the last successful run
      ∪ records referenced by review_queue rows resolved since the last successful run

partners        = { r' : an assertion-adjusted edge (r, r') exists with p >= auto_merge, r ∈ seed }
affected_entities = current entity of each record in (seed ∪ partners), where assigned
affected_nodes  = all current members of affected_entities ∪ seed ∪ partners
```

Seeding only from the batch is the specific loss the gap report names: it makes
`split_scenario` unexecutable in incremental mode, because a `never` asserted on two
records that appear in no batch reaches no run at all — while S4.4 requires assertions
to be "applied identically in incremental and full modes". So this module implements
each arm separately and names it, rather than computing one union nobody can audit.

**Five decisions this module makes, none of which is re-derivable from the arms.**

* **Partners come from EDGES, never from blocking co-keys.** S4.3.4 does call the
  blocking-key relation "the driver for the S4.5 touched-subgraph computation", and it
  is tempting to read that as "co-blocked records are partners". The normative formula is
  edge-based, and the two differ: blocking is a recall device that pairs records which
  then score at 0.02, so reading it here would widen the affected set to every record
  sharing a postcode prefix. This module therefore does not mention that relation at all
  — not in a query, not in a comment — and
  `tests/unit/entities/test_affected_nodes.py` greps the file to keep it that way.
* **The gray band creates no partners.** The bound is `p >= auto_merge` and the band is
  half-open (S4.3), so a pair at exactly `auto_merge` is a partner and one just below it
  is not. A banded pair is queued for review and "is **not** clustered" (S4.3.5); making
  it widen the affected set would cluster its entity on its account.
* **The assertion arm reads the DELTA, not the active set.** Created *or retracted*
  since the watermark: a retraction is exactly as much of a change as a creation, and
  reading `active` alone would make a retracted `never` invisible to the run that has to
  undo the cut it caused. This is the arm that makes a `never` on two non-batch records
  executable incrementally.
* **Tombstones read `raw_records`, not `int_std_records`.** S4.2 excludes a tombstoned
  record from the standardized corpus entirely, and S4.5.5 nevertheless requires its
  entity to enter the affected set so that `member_removed` / `split` / `retired` can be
  emitted. A tombstone arm reading the standardized view would find nothing, every time.
* **The widening is one hop through entities, and never through edges.** `affected_nodes`
  is the members of the entities of `seed ∪ partners` — not the transitive closure of the
  edge set. INV-EQ's proof sketch depends on exactly this: it needs "both endpoint
  components of every new edge", which one hop of entity membership delivers, and a
  transitive edge closure would drag in the whole component graph for no additional
  guarantee.

**Three decisions the EDGE set makes**, and each of them is a way an implementer loses a
partition rather than a way to write the query more tidily.

* **ALL currently-active edges among the members, never this run's scored pairs.**
  `match_scores` is cumulative and is never truncated per run (S4.3.4), so an entity
  whose edges were written by a run three weeks ago has no row from this one — and a
  loader scoped to `run_id` would return an empty edge set for it and fragment it into
  singletons. S4.5.1 states this as the one correct reading, in the same breath as the
  misreading. Which is why :func:`affected_edges` takes no `run_id` at all: it reads
  through :func:`er.matching.edges.current_edges`, whose scope is the run's
  `(model_version, tf_snapshot_id)` and nothing narrower.
* **The `cut_edges` exclusion is unconditional and applies on every later run.** Without
  it every cut is silently re-merged next run from the cumulative table and `never`
  becomes a no-op with a one-run half-life (S4.4.2). A cut is released by deactivating
  its row — retraction is ER-076's to write — so what is honoured here is `active`, and
  the pair reappears in the edge set the moment it is not.
* **Assertion edges exist only in memory, and only between affected nodes.** S4.4 is
  explicit that they are "never persisted to `match_scores`", so
  :func:`adjust_edges_with_assertions` is a function over a loaded list and issues no
  statement whatsoever. It is passed the node set because an active `always` between two
  records this run never touched would otherwise drag an edge — and with it two
  unclustered endpoints — into a subgraph the node formula deliberately left out.

**The loop the two sets feed** is S4.5.2's, and :func:`label_propagate` is it: `label(v) =
min(record_key)` over the closed neighbourhood of `v`, propagated to fixpoint, bounded by
`clustering.max_iterations` and failing hard when it does not settle. Three decisions
there, and each is a way the partition or the snapshot history goes wrong:

* **Every round ends in a pointer jump.** The closed-neighbourhood min alone moves a label
  one hop per round, so a component that is a path of `n` records would need `n` rounds and
  a 1024-record chain would blow the default cap of 50. Composing the round's own result
  with itself — `label(v) := L(L(v))` — doubles the reach each round instead, which is what
  makes `ceil(log2 n) + 1` (:func:`MAX_ITERATION_BOUND`) a true bound and the configured
  cap a safety net rather than the expected count.
* **The iterations run in the in-memory database and NOTHING per-round reaches the lake**
  (S4.0b, M17). The tables the loop rewrites each round are `TEMP`, so a round commits no
  DuckLake snapshot; a loop that materialised its state in `lake.main` would leave one
  snapshot per iteration between the endpoints `run_stages` recorded, and the S4.7
  recovery story reads that range.
* **Non-convergence is a hard failure, never a partial partition.** S4.5.2 and S13 both
  say so: exit `1`, `error_class = non_convergence` (S4.7), no snapshot, no events, and the
  unconverged component's size and minimum `record_key` logged.

**Shape.** A pure core — :func:`partners_of`, :func:`affected_nodes` and
:func:`adjust_edges_with_assertions` take already loaded rows and touch no connection —
plus thin loaders that read the relations. The core is what the unit layer exercises on a
bare runner (S8.1), and it is also what makes the S4.5.4 determinism argument checkable:
given the same rows it returns the same sets. :func:`label_propagate` sits between the
two: it needs a connection because S4.5.2 pins the loop to DuckDB SQL, but the connection
it needs is a bare `duckdb.connect()` — no lake, so the unit layer exercises it too.

**Scope, deliberately.** This module produces the node set and the edge set the
clustering runs over, and propagates labels over them. Splink parity for the full path is
ER-072's; the raw cluster -> `entity_id` mapping, INV-PERM and the events are ER-073's and
ER-074's; the exit-`10` on an empty affected set is ER-074's too; and
the `cut_edges` rows themselves — S4.4.2's cut choice and its bounded fixpoint — are
ER-076's, and all that happens here is the exclusion of rows already written. Edge
invalidation on supersession or deletion (S4.5.5) is ER-082's and ER-083's; this module
only honours the `is_active` flag it finds. Nothing here writes, invalidates or deletes a
row.
"""

from __future__ import annotations

from collections.abc import Iterable, Mapping, Sequence
from dataclasses import dataclass
from datetime import datetime
from typing import Any, Final

import duckdb

from er.entities.ids import canonicalize_pair, record_key
from er.errors import NonConvergenceError
from er.lake.model import SCHEMA_QUALIFIER
from er.matching.edges import current_edges
from er.review.assertions import ALWAYS, NEVER, Assertion
from er.review.queue import ENTITY, PAIR, RESOLVED_STATUSES

__all__ = [
    "ASSERTION_EDGE_PROBABILITY",
    "ASSERTION_EVIDENCE_SOURCE",
    "LABEL_PROP_ITERATIONS",
    "MAX_ITERATION_BOUND",
    "RECONCILE_STAGE",
    "SEED_ARMS",
    "SUCCEEDED",
    "AffectedSet",
    "Edge",
    "LabelPropagationResult",
    "SeedRecords",
    "adjust_edges_with_assertions",
    "affected_edges",
    "affected_nodes",
    "current_membership",
    "label_propagate",
    "last_reconciled_watermark",
    "load_affected_set",
    "partners_of",
    "seed_records",
]

#: The stage whose success defines "the last successful run" every delta is measured
#: against. S4.5.1 phrases each arm as "since the last successful run", and the run that
#: matters is the last one that *reconciled*: a run that ingested and then failed in
#: `match` left the corpus un-reconciled, so its deltas are still outstanding and a
#: watermark taken from it would silently drop them.
RECONCILE_STAGE: Final = "reconcile"

#: `run_stages.status` for a stage that finished (S5). Spelled from the vocabulary in
#: `er.lake.model.RUN_STATUSES` rather than as a literal at each use.
SUCCEEDED: Final = "succeeded"

#: The probability an active `always` assertion contributes to the edge set (S4.4).
#: `1.0` is also `clustering.cut_protect_probability`'s default, which is what makes an
#: assertion edge uncuttable — the coupling is stated in S4.4.2 and is the reason this
#: is a named constant rather than a literal.
ASSERTION_EDGE_PROBABILITY: Final = 1.0

#: `evidence.source` on an injected `always` edge. S4.4 writes the payload literally —
#: `{"source": "assertion", "assertion_id": …}` — and this value is what a consumer
#: discriminates on: an edge carrying it was never scored, has no `match_scores` row
#: behind it, and must never be given one.
ASSERTION_EVIDENCE_SOURCE: Final = "assertion"

#: The five arms of S4.5.1's `seed`, in the order the spec writes them. Exported so a
#: caller (and :class:`SeedRecords`) can enumerate them rather than restate them.
SEED_ARMS: Final[tuple[str, ...]] = (
    "batch",
    "content_hash_delta",
    "deletions",
    "assertion_delta",
    "review_delta",
)

#: The name S4.5.6 gives the propagation's round count in `run_stages.counters`, and the
#: name `er.obs.counters.DECLARED_COUNTERS['reconcile']` already carries. Exported so
#: ER-074 spells it once — a stage that invented `label_prop_iters` here would satisfy
#: every test in this module and still write a payload S5.2 calls incomplete.
LABEL_PROP_ITERATIONS: Final = "label_prop_iterations"

_RAW_RECORDS: Final = f"{SCHEMA_QUALIFIER}.raw_records"
_INGEST_BATCHES: Final = f"{SCHEMA_QUALIFIER}.ingest_batches"
_ASSERTIONS: Final = f"{SCHEMA_QUALIFIER}.assertions"
_REVIEW_QUEUE: Final = f"{SCHEMA_QUALIFIER}.review_queue"
_ENTITY_MEMBERSHIP: Final = f"{SCHEMA_QUALIFIER}.entity_membership"
_RUNS: Final = f"{SCHEMA_QUALIFIER}.runs"
_RUN_STAGES: Final = f"{SCHEMA_QUALIFIER}.run_stages"
_CUT_EDGES: Final = f"{SCHEMA_QUALIFIER}.cut_edges"
#: The standardized corpus, which is where S4.2 puts a record that is NOT tombstoned.
#: The edge set restricts to it, so an edge incident to a tombstone is excluded even
#: while its `match_scores` row is still `is_active` (S4.5.1, S4.5.5).
_INT_STD_RECORDS: Final = f"{SCHEMA_QUALIFIER}.int_std_records"


@dataclass(frozen=True)
class SeedRecords:
    """S4.5.1's `seed`, kept as its five arms rather than as one union.

    The union is what the formula consumes and :attr:`records` is it; the arms are
    retained because they are what a stage reports and what an operator debugging an
    unexpectedly wide — or unexpectedly empty — affected set has to look at. A seed
    delivered as a single set makes "why is this record here?" unanswerable without
    re-running the five queries by hand.
    """

    batch: frozenset[str]
    content_hash_delta: frozenset[str]
    deletions: frozenset[str]
    assertion_delta: frozenset[str]
    review_delta: frozenset[str]

    @property
    def records(self) -> frozenset[str]:
        """The union of the five arms: S4.5.1's `seed`."""
        return frozenset().union(*(getattr(self, arm) for arm in SEED_ARMS))

    def arm_sizes(self) -> dict[str, int]:
        """Each arm's cardinality, for the stage's counters and for a diagnosis."""
        return {arm: len(getattr(self, arm)) for arm in SEED_ARMS}


@dataclass(frozen=True)
class AffectedSet:
    """The result of S4.5.1's node formula, with the entities it widened through.

    `entities` is carried alongside `nodes` because the two answer different questions
    and neither is derivable from the other here: reconciliation needs the prior
    partition of the affected entities (INV-PERM, S4.5.3), and an entity that is affected
    while holding no member — one whose every record was tombstoned — has to be reachable
    so it can be retired.
    """

    seed: frozenset[str]
    partners: frozenset[str]
    entities: frozenset[str]
    nodes: frozenset[str]

    @property
    def is_empty(self) -> bool:
        """Whether this run has nothing to reconcile (S4.5.1, and S4.0's exit ``10``).

        Both halves, because an entity may be affected with no surviving member: a set
        judged empty on `nodes` alone would skip the retirement that entity is owed.
        """
        return not self.nodes and not self.entities


@dataclass(frozen=True)
class Edge:
    """One edge of the clustering edge set: a scored pair, or an assertion-sourced one.

    A record rather than the `(rec_a_key, rec_b_key, match_probability)` triple
    :func:`~er.matching.edges.current_edges` returns, for one reason: after S4.4's
    adjustment the set holds edges of two provenances, and the injected ones carry an
    `evidence` payload with no `match_scores` row behind it. A triple cannot say that, and
    a consumer that has to infer "assertion" from `match_probability == 1.0` would also
    call a genuinely certain scored pair an assertion.

    The pair is validated as canonical at construction. S5.0 canonicalises at write time
    through one helper and "readers never perform a two-sided join", so a non-canonical
    edge here is not something to sort into shape — it means some producer bypassed
    :func:`~er.entities.ids.canonicalize_pair`, and the pairs it made are not addressable
    by the key everything downstream joins on.
    """

    rec_a_key: str
    rec_b_key: str
    match_probability: float
    #: S4.4's payload on an injected edge, and ``None`` on a scored one. ``None`` rather
    #: than the scored row's own `evidence`: S4.5.1's normative query projects three
    #: columns, and the waterfall stays in `match_scores` where S4.3.5 keeps it.
    evidence: Mapping[str, str] | None = None

    def __post_init__(self) -> None:
        if canonicalize_pair(self.rec_a_key, self.rec_b_key) != self.pair:
            raise ValueError(
                f"({self.rec_a_key!r}, {self.rec_b_key!r}) is not canonical: S5.0 requires "
                f"rec_a_key < rec_b_key on every pair, written through "
                f"er.entities.ids.canonicalize_pair"
            )

    @property
    def pair(self) -> tuple[str, str]:
        """The canonical pair this edge joins."""
        return (self.rec_a_key, self.rec_b_key)

    @property
    def is_assertion(self) -> bool:
        """Whether S4.4's adjustment injected this edge rather than a scorer writing it.

        The check a writer makes before it persists anything: an assertion edge has no
        `match_scores` row and may never be given one, and `assertions` is its durable
        record (S4.4).
        """
        return self.evidence is not None and (
            self.evidence.get("source") == ASSERTION_EVIDENCE_SOURCE
        )


def _keys(rows: Sequence[Sequence[Any]]) -> frozenset[str]:
    """`(source_system, source_record_id)` rows as S5.0 record keys.

    Through :func:`~er.entities.ids.record_key` rather than a SQL `||` concatenation, so
    the one implementation of D6's scalar identity — including its ban on `':'` in either
    component — is what every arm here produces.
    """
    return frozenset(record_key(str(system), str(identifier)) for system, identifier in rows)


def _pair_keys(rows: Sequence[Sequence[Any]]) -> frozenset[str]:
    """Both endpoints of every `(rec_a_key, rec_b_key)` row, NULLs dropped.

    `review_queue.rec_a_key` is nullable — it is non-NULL only for
    `subject_type='pair'` (S5) — and a NULL endpoint is not a record reference.
    """
    return frozenset(str(key) for row in rows for key in row if key is not None)


def _since(column: str, watermark: datetime | None) -> tuple[str, list[Any]]:
    """A `since the watermark` predicate for ``column``, and the parameters it binds.

    The unbounded case is rendered as a literal `TRUE` rather than as a bound `NULL`
    that the predicate tests for. Two reasons, and the first is fatal: DuckDB cannot
    infer a type for a bare ``?`` appearing only in ``? IS NULL``, so the parameterised
    spelling fails to bind at all. The second is that `TRUE` is what "no cutoff" means,
    and a reader of the generated SQL should not have to evaluate a NULL test to see it.
    """
    if watermark is None:
        return "TRUE", []
    return f"{column} > ?", [watermark]


def last_reconciled_watermark(connection: duckdb.DuckDBPyConnection) -> datetime | None:
    """The `started_at` of the most recent run whose `reconcile` stage succeeded (S5.2).

    THE cutoff. S4.5.1 phrases four of its five arms as "since the last successful run",
    and every one of them takes this value: an arm computing its own cutoff — from
    `max(ingested_at)`, say, or from the previous `runs` row regardless of outcome —
    would disagree with the others about which changes are outstanding, and the
    disagreement would show up as a record silently missing from the seed.

    `started_at` rather than `ended_at`, and deliberately: anything written *while* the
    last reconcile was running was not seen by it, because the affected set it clustered
    was computed at its own start. A watermark at `ended_at` would swallow exactly those
    rows.

    Returns:
        The instant, or ``None`` when no run has ever reconciled successfully. ``None``
        means "unbounded": every delta arm then matches its whole relation, which is the
        correct reading of a first run — nothing has been reconciled, so everything is
        outstanding.
    """
    row = connection.execute(
        f"SELECT max(runs.started_at) "
        f"  FROM {_RUNS} AS runs "
        f"  JOIN {_RUN_STAGES} AS stages ON stages.run_id = runs.run_id "
        f" WHERE stages.stage = ? AND stages.status = ?",
        [RECONCILE_STAGE, SUCCEEDED],
    ).fetchone()
    if row is None or row[0] is None:
        return None
    watermark = row[0]
    assert isinstance(watermark, datetime)
    return watermark


def _batch_arm(connection: duckdb.DuckDBPyConnection, run_id: str) -> frozenset[str]:
    """Arm 1: the records this run's ingest batches delivered.

    Scoped by `run_id` through `ingest_batches` rather than by a timestamp, because a
    run is the unit S4.5.1 names and two runs may deliver in the same second. Tombstone
    rows carry an `ingest_batch_id` like any other version (S4.1.1) and are therefore in
    this arm too when the run tombstoned them — the deletions arm exists for the ones
    tombstoned by an *earlier* run that never reconciled.
    """
    rows = connection.execute(
        f"SELECT DISTINCT source_system, source_record_id "
        f"  FROM {_RAW_RECORDS} "
        f" WHERE ingest_batch_id IN (SELECT ingest_batch_id FROM {_INGEST_BATCHES} "
        f"                            WHERE run_id = ?)",
        [run_id],
    ).fetchall()
    return _keys(rows)


def _content_hash_arm(
    connection: duckdb.DuckDBPyConnection, watermark: datetime | None
) -> frozenset[str]:
    """Arm 2: records whose `content_hash` changed since the watermark.

    `raw_records` is append-only version history (D7), so a change is a version appended
    since the watermark for a key that already carried a *different* hash. Both halves
    are load-bearing: without the second, every record of a first delivery would count as
    changed; without the first, a key re-delivered at its known hash would — and S4.1
    counts that one `unchanged`, appending nothing at all.

    A superseded record is what S4.5.5 invalidates edges for, and INV-SCORE (S4.3.3) is
    stated over the endpoint hashes precisely because a changed hash makes the pair a
    different scoring problem. That is why the arm exists at all.
    """
    since, parameters = _since("appended.ingested_at", watermark)
    rows = connection.execute(
        f"SELECT DISTINCT appended.source_system, appended.source_record_id "
        f"  FROM {_RAW_RECORDS} AS appended "
        f" WHERE NOT appended.is_deleted "
        f"   AND {since} "
        f"   AND EXISTS (SELECT 1 FROM {_RAW_RECORDS} AS prior "
        f"                WHERE prior.source_system = appended.source_system "
        f"                  AND prior.source_record_id = appended.source_record_id "
        f"                  AND prior.content_hash <> appended.content_hash "
        f"                  AND prior.ingested_at <= appended.ingested_at)",
        parameters,
    ).fetchall()
    return _keys(rows)


def _deletions_arm(
    connection: duckdb.DuckDBPyConnection, watermark: datetime | None
) -> frozenset[str]:
    """Arm 3: records tombstoned or resurrected since the watermark (S4.1.1, S4.5.5).

    Both directions in one arm, as S4.5.1 writes them, because they are one event class:
    the record's liveness changed, so its entity has to be re-clustered either to lose it
    or to take it back. A resurrection is recognised by an ordinary content version
    appended after a tombstone for the same key — no flag, because S4.1.1 is explicit
    that resurrection "is an ordinary new record ... with no special case".

    Read from `raw_records` and not from `int_std_records`: S4.2 excludes a tombstoned
    record from the standardized corpus entirely, so the standardized view is precisely
    where a tombstone is *not* visible.
    """
    buried_since, buried_parameters = _since("tombstone.ingested_at", watermark)
    revived_since, revived_parameters = _since("revived.ingested_at", watermark)
    rows = connection.execute(
        f"SELECT DISTINCT source_system, source_record_id FROM ("
        f"  SELECT tombstone.source_system, tombstone.source_record_id "
        f"    FROM {_RAW_RECORDS} AS tombstone "
        f"   WHERE tombstone.is_deleted "
        f"     AND {buried_since} "
        f"  UNION ALL "
        f"  SELECT revived.source_system, revived.source_record_id "
        f"    FROM {_RAW_RECORDS} AS revived "
        f"   WHERE NOT revived.is_deleted "
        f"     AND {revived_since} "
        f"     AND EXISTS (SELECT 1 FROM {_RAW_RECORDS} AS buried "
        f"                  WHERE buried.source_system = revived.source_system "
        f"                    AND buried.source_record_id = revived.source_record_id "
        f"                    AND buried.is_deleted "
        f"                    AND buried.ingested_at < revived.ingested_at)"
        f") AS changed_liveness",
        [*buried_parameters, *revived_parameters],
    ).fetchall()
    return _keys(rows)


def _assertion_arm(
    connection: duckdb.DuckDBPyConnection, watermark: datetime | None
) -> frozenset[str]:
    """Arm 4: records referenced by assertions created OR retracted since the watermark.

    The delta, not the active set. `er reconcile` "reads the pending assertion delta
    itself" (S4.5.1), and a retraction has to be in it: retracting a `never` releases the
    cut it caused (S4.4.2), which is a change to the partition that no other arm would
    ever surface. S4.4 keeps retracted rows precisely so this is computable — nothing
    deletes an `assertions` row.
    """
    created_since, created_parameters = _since("created_at", watermark)
    retracted_since, retracted_parameters = _since("retracted_at", watermark)
    rows = connection.execute(
        f"SELECT rec_a_key, rec_b_key FROM {_ASSERTIONS} "
        f" WHERE {created_since} "
        f"    OR (retracted_at IS NOT NULL AND {retracted_since})",
        [*created_parameters, *retracted_parameters],
    ).fetchall()
    return _pair_keys(rows)


def _review_arm(
    connection: duckdb.DuckDBPyConnection, watermark: datetime | None
) -> frozenset[str]:
    """Arm 5: records referenced by `review_queue` rows resolved since the watermark.

    "Resolved" is all three settled statuses of S4.3.5 — `resolved_match`,
    `resolved_no_match` and `dismissed` — and not only the two that write an assertion.
    A dismissal is a steward decision about the pair, and the arm is defined by the row
    being settled rather than by what it settled to.

    A `subject_type='entity'` row (the S4.4.2 escalation's sibling, and S11's coherence
    findings) references its entity's current members, which is what "records referenced
    by" means for a row carrying no pair. It is folded in here rather than left to the
    widening, because an entity whose members are all elsewhere unaffected would
    otherwise not reach the seed at all.
    """
    settled = sorted(RESOLVED_STATUSES)
    placeholders = ", ".join("?" for _ in settled)
    since, parameters = _since("resolved_at", watermark)
    rows = connection.execute(
        f"SELECT rec_a_key, rec_b_key FROM {_REVIEW_QUEUE} "
        f" WHERE subject_type = ? AND status IN ({placeholders}) "
        f"   AND resolved_at IS NOT NULL AND {since}",
        [PAIR, *settled, *parameters],
    ).fetchall()
    members = connection.execute(
        f"SELECT membership.source_system, membership.source_record_id "
        f"  FROM {_ENTITY_MEMBERSHIP} AS membership "
        f" WHERE membership.entity_id IN ("
        f"   SELECT entity_id FROM {_REVIEW_QUEUE} "
        f"    WHERE subject_type = ? AND entity_id IS NOT NULL "
        f"      AND status IN ({placeholders}) "
        f"      AND resolved_at IS NOT NULL AND {since})",
        [ENTITY, *settled, *parameters],
    ).fetchall()
    return _pair_keys(rows) | _keys(members)


def seed_records(
    connection: duckdb.DuckDBPyConnection,
    *,
    run_id: str,
    watermark: datetime | None,
) -> SeedRecords:
    """S4.5.1's five seed arms, each queried and kept separately.

    Args:
        connection: a connection with the lake attached (S4.0b). Nothing is written.
        run_id: this run's id, which selects the batch arm through `ingest_batches`.
        watermark: the cutoff from :func:`last_reconciled_watermark`. ``None`` leaves
            the four delta arms unbounded, which is what a first run means. It is a
            required argument rather than a default so no caller can compute the seed
            against a cutoff it never chose — the four arms must share one.

    Returns:
        The arms, unioned by :attr:`SeedRecords.records`.
    """
    return SeedRecords(
        batch=_batch_arm(connection, run_id),
        content_hash_delta=_content_hash_arm(connection, watermark),
        deletions=_deletions_arm(connection, watermark),
        assertion_delta=_assertion_arm(connection, watermark),
        review_delta=_review_arm(connection, watermark),
    )


def adjust_edges_with_assertions(
    edges: Iterable[Edge],
    assertions: Iterable[Assertion],
    *,
    nodes: Iterable[str] | None = None,
) -> list[Edge]:
    """S4.4's edge adjustment: minus active `never`, plus active `always` at `p = 1.0`.

    PURE, and that is normative rather than convenient. S4.4 states the adjustment "is
    not a SQL clause and is not part of the `select`": the query loads the scored edges
    and this runs over the loaded result. Nothing here executes a statement, so no
    assertion edge can reach `match_scores` — `assertions` is their durable record, and
    every `match_scores` row stays a scored one with `model_version` and `tf_snapshot_id`
    `NOT NULL` (S5).

    `always` first and `never` second, which is the order S4.4 fixes so that the two
    precedence readings cannot disagree: a pair carrying both is removed either way.
    S4.4 also rejects that pair at write time (exit ``1``), so the ordering here is what
    makes the claim true of *any* assertion set — including one a future writer, or a
    direct insert, produced.

    Only ACTIVE assertions adjust anything. A retracted row is a historical record and
    not a constraint; it reaches the seed through the assertion delta arm (S4.5.1) and
    stops there.

    Args:
        edges: the loaded edge set; :func:`affected_edges`' result in production.
        assertions: the assertion set, typically
            :func:`~er.review.assertions.active_assertions`'.
        nodes: the affected node set to confine INJECTION to. An `always` with an
            endpoint outside it is skipped, because an edge between two records this run
            never touched would pull an unclustered endpoint into a subgraph S4.5.1's
            node formula deliberately left out. ``None`` injects unconditionally, which
            is what the partner rule needs: a partner IS a record an `always` reaches
            from the seed, so filtering by a node set that rule is still computing would
            be circular. Removal by `never` is unconditional either way — a pair outside
            the node set has no edge in `edges` to remove.

    Returns:
        The adjusted edges in canonical pair order, one per pair.
    """
    adjusted = {edge.pair: edge for edge in edges}
    active = [assertion for assertion in assertions if assertion.active]
    reachable = None if nodes is None else frozenset(nodes)
    for assertion in active:
        if assertion.kind != ALWAYS:
            continue
        if reachable is not None and not reachable.issuperset(assertion.pair):
            continue
        adjusted[assertion.pair] = Edge(
            *assertion.pair,
            match_probability=ASSERTION_EDGE_PROBABILITY,
            evidence={
                "source": ASSERTION_EVIDENCE_SOURCE,
                "assertion_id": assertion.assertion_id,
            },
        )
    for assertion in active:
        if assertion.kind == NEVER:
            adjusted.pop(assertion.pair, None)
    return [adjusted[pair] for pair in sorted(adjusted)]


def _adjusted_edges(
    edges: Iterable[tuple[str, str, float]],
    assertions: Iterable[Assertion],
) -> dict[tuple[str, str], float]:
    """The partner rule's view of :func:`adjust_edges_with_assertions`: pair -> `p`.

    A projection of the one adjustment implementation and not a second one — two spellings
    of "minus `never`, plus `always`" would be two chances to disagree about precedence,
    and the partner rule and the clustering edge set are required to see the same adjusted
    graph (S4.5.1).

    The triples are canonicalised on the way in rather than trusted. A caller holding
    `(b, a)` would otherwise miss an assertion for `(a, b)` on a dict lookup — a `never`
    that silently failed to remove its edge — because :class:`Edge` and
    :class:`~er.review.assertions.Assertion` are both keyed on the canonical pair (S5.0).

    No node set is passed: see :func:`adjust_edges_with_assertions`.
    """
    adjusted = adjust_edges_with_assertions(
        [
            Edge(*canonicalize_pair(rec_a_key, rec_b_key), probability)
            for rec_a_key, rec_b_key, probability in edges
        ],
        assertions,
    )
    return {edge.pair: edge.match_probability for edge in adjusted}


def partners_of(
    seed: Iterable[str],
    edges: Iterable[tuple[str, str, float]],
    *,
    auto_merge: float,
    assertions: Iterable[Assertion] = (),
) -> frozenset[str]:
    """The far ends of assertion-adjusted edges at or above `auto_merge` (S4.5.1).

    PURE: it opens no connection and reads no relation. `edges` is the loaded edge set —
    :func:`er.matching.edges.current_edges`' result in production, a literal list in the
    unit layer.

    The bound is inclusive and the band is half-open (S4.3), so a pair at exactly
    `auto_merge` yields a partner and one at `auto_merge - ε` does not. That is not a
    rounding detail: a gray-band pair is queued for review and is not clustered (S4.3.5),
    and letting it produce a partner would pull its entity into the run on the strength
    of a pair a steward has not yet ruled on.

    Args:
        seed: S4.5.1's `seed`.
        edges: `(rec_a_key, rec_b_key, match_probability)` triples, canonical per S5.0.
        auto_merge: `thresholds.auto_merge`, which IS the clustering cut (D13).
        assertions: the assertion set to adjust with; typically
            :func:`~er.review.assertions.active_assertions`'. An active `always` supplies
            a partner even where nothing was ever scored — which is the case a
            steward-authored merge of two never-blocked records depends on — and an
            active `never` withdraws one.

    Returns:
        The partner records, **excluding** anything already in `seed`. Disjoint from the
        seed so that the two sets can be reported and reasoned about separately; the
        formula consumes their union either way.
    """
    seeded = frozenset(seed)
    partners: set[str] = set()
    for (rec_a_key, rec_b_key), probability in _adjusted_edges(edges, assertions).items():
        if probability < auto_merge:
            continue
        if rec_a_key in seeded:
            partners.add(rec_b_key)
        if rec_b_key in seeded:
            partners.add(rec_a_key)
    return frozenset(partners) - seeded


def affected_nodes(
    seed: Iterable[str],
    *,
    edges: Iterable[tuple[str, str, float]],
    membership: Mapping[str, str],
    auto_merge: float,
    assertions: Iterable[Assertion] = (),
) -> AffectedSet:
    """S4.5.1's node formula, evaluated over already-loaded rows.

    PURE, and the reason the S4.5.4 determinism claim is checkable: the same rows give
    the same four sets, with no dependence on scan order because every result is a set.

    The widening is **one hop through entity membership and no hops through edges**. A
    record's entity brings in that entity's current members; a member's own partners do
    not then bring in theirs. INV-EQ's proof sketch needs exactly this much — "both
    endpoint components of every new edge" — and a transitive edge closure would buy
    nothing while making the affected set of a well-connected corpus the whole corpus.

    Args:
        seed: S4.5.1's `seed`; :attr:`SeedRecords.records` in production.
        edges: the loaded edge set, for :func:`partners_of`.
        membership: `record_key -> entity_id` for the CURRENT partition (S4.5.3). It must
            be complete for the entities of `seed ∪ partners` — every member of them, not
            only the seeded ones — since those members are what the widening returns.
            :func:`current_membership` loads exactly that. A record absent from the
            mapping has no entity, which is the "where assigned" of the formula.
        auto_merge: `thresholds.auto_merge`.
        assertions: the assertion set the edges are adjusted with.

    Returns:
        The seed, the partners, the affected entities and the affected nodes. Every
        field is empty when the seed is — a run with no batch, no delta and no
        resolution has nothing to reconcile, which is S4.0's exit ``10`` (ER-074's).
    """
    seeded = frozenset(seed)
    partners = partners_of(seeded, edges, auto_merge=auto_merge, assertions=assertions)
    reached = seeded | partners
    entities = frozenset(membership[key] for key in reached if key in membership)
    co_members = frozenset(key for key, entity_id in membership.items() if entity_id in entities)
    return AffectedSet(
        seed=seeded,
        partners=partners,
        entities=entities,
        nodes=reached | co_members,
    )


def current_membership(
    connection: duckdb.DuckDBPyConnection, records: Iterable[str]
) -> dict[str, str]:
    """`record_key -> entity_id` for `records` AND for every co-member of their entities.

    Two queries rather than one join, because the two answer different questions and the
    second depends on the first's *result*: which entities are affected, and then who
    else is in them. Loading only the requested records would make
    :func:`affected_nodes` unable to widen at all — it would see each seed record's
    entity and none of that entity's other members, which is the exact bug S4.5.1's
    "all current members of affected_entities" exists to prevent.

    `entity_membership` is CURRENT STATE, one row per record (D3), so the result is a
    mapping and not a multimap. `entities.merged_into` is deliberately not consulted: it
    resolves external ids only and is "**never** used to resolve current membership"
    (S4.5.3).

    Returns:
        The mapping, empty when nothing requested is assigned.
    """
    requested = sorted(set(records))
    if not requested:
        return {}
    entities = connection.execute(
        f"SELECT DISTINCT entity_id FROM {_ENTITY_MEMBERSHIP} "
        f" WHERE record_key IN ({', '.join('?' for _ in requested)})",
        requested,
    ).fetchall()
    affected = sorted({str(entity_id) for (entity_id,) in entities})
    if not affected:
        return {}
    rows = connection.execute(
        f"SELECT record_key, entity_id FROM {_ENTITY_MEMBERSHIP} "
        f" WHERE entity_id IN ({', '.join('?' for _ in affected)})",
        affected,
    ).fetchall()
    return {str(key): str(entity_id) for key, entity_id in rows}


def load_affected_set(
    connection: duckdb.DuckDBPyConnection,
    *,
    run_id: str,
    auto_merge: float,
    edges: Iterable[tuple[str, str, float]],
    assertions: Iterable[Assertion],
    watermark: datetime | None = None,
) -> AffectedSet:
    """The loader arm: read the relations, then evaluate the pure core over them.

    `edges` and `assertions` are passed in rather than read here, because both already
    have exactly one reader — :func:`er.matching.edges.current_edges` and
    :func:`er.review.assertions.active_assertions` — and a second call site choosing its
    own filters is how two parts of one stage end up clustering different graphs.

    Args:
        connection: a connection with the lake attached (S4.0b). Nothing is written.
        run_id: this run's id, for the batch arm.
        auto_merge: `thresholds.auto_merge`.
        edges: the current edge set, loaded at this run's `(model_version,
            tf_snapshot_id)`.
        assertions: the active assertion set.
        watermark: an explicit cutoff; :func:`last_reconciled_watermark` supplies it
            when omitted. Overridable so a caller that has already taken the watermark —
            a stage that reports it, or a re-resolution pinned to an earlier one — does
            not take a second, possibly different, reading.

    Returns:
        The :class:`AffectedSet` for this run.
    """
    cutoff = last_reconciled_watermark(connection) if watermark is None else watermark
    seed = seed_records(connection, run_id=run_id, watermark=cutoff).records
    # The partner set is needed before the membership load — a partner's entity is
    # affected, so its co-members are affected too — and `affected_nodes` recomputes it
    # from the same rows. Recomputing is cheap and keeps the core a function of its
    # arguments alone; threading a partner set into it would make the two answers
    # capable of disagreeing.
    partners = partners_of(seed, edges, auto_merge=auto_merge, assertions=assertions)
    return affected_nodes(
        seed,
        edges=edges,
        membership=current_membership(connection, seed | partners),
        auto_merge=auto_merge,
        assertions=assertions,
    )


def _active_cut_pairs(connection: duckdb.DuckDBPyConnection) -> frozenset[tuple[str, str]]:
    """Every pair S4.4.2 has cut and not released, as canonical pairs.

    Unfiltered by `(model_version, tf_snapshot_id)`, exactly as S4.5.1's query writes the
    exclusion, and the columns permit it: `cut_edges` makes both of them nullable (S5)
    because a cut is a statement about a *pair*, not about a scoring. A reader that
    scoped the exclusion to the run's key would re-merge every cut the moment a new model
    was trained — which is the one-run half-life S4.4.2 exists to prevent, arriving a
    model version later.
    """
    rows = connection.execute(
        f"SELECT rec_a_key, rec_b_key FROM {_CUT_EDGES} WHERE active"
    ).fetchall()
    return frozenset((str(rec_a_key), str(rec_b_key)) for rec_a_key, rec_b_key in rows)


def _standardized_records(
    connection: duckdb.DuckDBPyConnection, records: Iterable[str]
) -> frozenset[str]:
    """Which of `records` `int_std_records` still holds — i.e. which are not tombstoned.

    Asked as "which of these", not "all of them", because the corpus is the whole lake
    and the question is only ever about the endpoints of a handful of candidate edges.

    S4.2 excludes a tombstoned record from the standardized corpus entirely, so absence
    here IS the tombstone: S4.5.1 restricts both endpoints to this relation, and S4.5.5
    calls the exclusion permanent for a tombstone precisely because the record cannot
    come back into it without being re-delivered.
    """
    requested = sorted(set(records))
    if not requested:
        return frozenset()
    rows = connection.execute(
        f"SELECT record_key FROM {_INT_STD_RECORDS} "
        f" WHERE record_key IN ({', '.join('?' for _ in requested)})",
        requested,
    ).fetchall()
    return frozenset(str(record_key_value) for (record_key_value,) in rows)


def affected_edges(
    connection: duckdb.DuckDBPyConnection,
    nodes: Iterable[str],
    *,
    model_version: str,
    tf_snapshot_id: str,
    auto_merge: float,
) -> list[Edge]:
    """S4.5.1's affected EDGE set: ALL currently-active edges among the affected nodes.

    NOT this run's scored pairs. `match_scores` is cumulative and is never truncated per
    run (S4.3.4), so an entity whose edges were written by an earlier run carries no row
    from this one; S4.5.1 says in as many words that an implementer who loads only this
    run's pairs "will spuriously fragment every touched entity". This function therefore
    takes no `run_id`, and the only scope it applies is the run's `(model_version,
    tf_snapshot_id)` — rows at another key are a different scoring problem (INV-SCORE,
    S4.3.3).

    The predicate list is S4.5.1's, sourced in one place each:
    :func:`~er.matching.edges.current_edges` applies the key, `is_active` and the
    inclusive `p >= auto_merge` bound and returns one row per canonical pair; both
    endpoints must be affected nodes; the pair must not be an active `cut_edges` row
    (S4.4.2); and both endpoints must still be in `int_std_records`, which excludes a
    tombstoned endpoint whose `match_scores` row is still `is_active` (S4.5.5).

    The result is NOT assertion-adjusted. :func:`adjust_edges_with_assertions` is the
    other half and is deliberately separate: it is pure, it is what the unit layer can
    exercise without a lake, and keeping it out of the query is how S4.4's "not a SQL
    clause" stays true by construction.

    Args:
        connection: a connection with the lake attached (S4.0b). Nothing is written.
        nodes: the affected node set — :attr:`AffectedSet.nodes`.
        model_version: the run's model version.
        tf_snapshot_id: the run's TF snapshot.
        auto_merge: `thresholds.auto_merge`, which IS the clustering cut (D13).

    Returns:
        The edges in canonical pair order, each pair at most once, every one satisfying
        `rec_a_key < rec_b_key`. Empty when the affected set is empty — a run with
        nothing to reconcile queries no relation at all.

    Raises:
        er.matching.edges.DuplicateEdgeKeyError: `match_scores` holds more than one row
            for a pair at this key (S5.0, S4.3.4).
    """
    affected = frozenset(nodes)
    if not affected:
        return []
    among = [
        (rec_a_key, rec_b_key, probability)
        for rec_a_key, rec_b_key, probability in current_edges(
            connection, model_version, tf_snapshot_id, min_probability=auto_merge
        )
        if rec_a_key in affected and rec_b_key in affected
    ]
    if not among:
        return []
    cut = _active_cut_pairs(connection)
    live = _standardized_records(
        connection, (key for rec_a_key, rec_b_key, _ in among for key in (rec_a_key, rec_b_key))
    )
    return [
        Edge(rec_a_key, rec_b_key, probability)
        for rec_a_key, rec_b_key, probability in among
        if (rec_a_key, rec_b_key) not in cut and rec_a_key in live and rec_b_key in live
    ]


#: The three relations the loop works in, every one of them `TEMP`. S4.0b binds the S4.5
#: propagation to the in-memory database — "iterations run in the in-memory database and
#: only the final labelling is written to the lake, so the loop cannot commit one snapshot
#: per iteration" — and a `TEMP` relation lives in the connection's `temp` catalog, which
#: is not `lake` and has no snapshot history at all. The names are prefixed because they
#: share a namespace with whatever else the run connection has materialised.
_LABELS_RELATION: Final = "er_label_prop_labels"
_ADJACENCY_RELATION: Final = "er_label_prop_adjacency"
_NEXT_RELATION: Final = "er_label_prop_next"

#: Dropped in this order — the derived relation first — on every exit path, including the
#: non-convergence raise. Nothing but the returned labelling leaves the function.
_LOOP_RELATIONS: Final[tuple[str, ...]] = (
    _NEXT_RELATION,
    _LABELS_RELATION,
    _ADJACENCY_RELATION,
)

#: One round of S4.5.2, as one statement.
#:
#: `closed` is the spec's `label(v) = min(record_key)` over the CLOSED neighbourhood: the
#: `LEFT JOIN` and the `coalesce` are what makes it closed, so a node with no incident edge
#: keeps its own label instead of dropping out of the result entirely (S4.5.3's "a record
#: leaving all clusters becomes a singleton" has no input if it does).
#:
#: The final `SELECT` is the pointer jump, and it is why the loop terminates in
#: :func:`MAX_ITERATION_BOUND` rounds rather than in one round per hop: composing the
#: round's own result with itself doubles the reach each time. `closed` is referenced
#: twice, which is exactly the composition — `label(v) := L(L(v))` — and never a second
#: pass over the edges.
#:
#: `min` over `VARCHAR` is lexicographic, which is the order S5.0's `record_key` is
#: canonicalised and compared in everywhere else in this module (`canonicalize_pair`), so
#: the label of a component is the same minimum a reader computing it in Python gets.
_ROUND_SQL: Final = f"""
CREATE OR REPLACE TEMP TABLE {_NEXT_RELATION} AS
WITH neighbourhood AS (
    SELECT adjacency.node AS record_key, min(labels.label) AS label
      FROM {_ADJACENCY_RELATION} AS adjacency
      JOIN {_LABELS_RELATION} AS labels ON labels.record_key = adjacency.neighbour
     GROUP BY adjacency.node
), closed AS (
    SELECT labels.record_key,
           least(labels.label, coalesce(neighbourhood.label, labels.label)) AS label
      FROM {_LABELS_RELATION} AS labels
      LEFT JOIN neighbourhood ON neighbourhood.record_key = labels.record_key
)
SELECT closed.record_key, coalesce(jumped.label, closed.label) AS label
  FROM closed
  LEFT JOIN closed AS jumped ON jumped.record_key = closed.label
"""

#: How many labels the round moved. Zero IS the fixpoint S4.5.2 propagates to: the round
#: is monotone — every label it writes is `<=` the one it replaces and is a record in the
#: same component — so a round that moves nothing can only be followed by rounds that move
#: nothing.
_CHANGED_SQL: Final = f"""
SELECT count(*)
  FROM {_NEXT_RELATION} AS next_labels
  JOIN {_LABELS_RELATION} AS labels USING (record_key)
 WHERE next_labels.label IS DISTINCT FROM labels.label
"""

_ADOPT_SQL: Final = (
    f"CREATE OR REPLACE TEMP TABLE {_LABELS_RELATION} AS "
    f"SELECT record_key, label FROM {_NEXT_RELATION}"
)


def MAX_ITERATION_BOUND(node_count: int) -> int:
    """`ceil(log2 n) + 1` — the rounds S4.5.2's pointer jumping needs for `n` nodes.

    Upper case because it is a bound of the ALGORITHM and not a knob: S4.5.2 states it as
    a fact about pointer jumping ("halves path length per round"), it is a function of `n`
    only, and no configuration can move it. `clustering.max_iterations` is the other
    number — the configured cap, the safety net, and the one whose exhaustion fails the
    stage — and the two are deliberately not the same thing (S6, default 50).

    The worst case for a component of `n` records is the path: the round's reach after `k`
    rounds is `2^(k+1) - 2` hops, so it covers a path's `n - 1` hops once
    `2^(k+1) >= n + 1`, and the round that then finds nothing to move is the `+ 1`. Any
    other shape of the same size has a smaller diameter and settles sooner.

    Computed with :meth:`int.bit_length` rather than `math.log2`: `ceil(log2 n)` is
    `(n - 1).bit_length()` exactly, for every `n`, with no float in the path — and this
    number is asserted against at `n = 1024`, where `log2` is exact but its neighbours
    are the sort of place a float bound goes off by one.

    Args:
        node_count: how many records are being propagated over. The whole node set is a
            sound argument even though the bound is per-component, because the bound is
            monotone in `n` and no component is larger than the set.

    Returns:
        The bound, and ``0`` for an empty node set — no round is run at all.
    """
    if node_count <= 0:
        return 0
    return (node_count - 1).bit_length() + 1


@dataclass(frozen=True)
class LabelPropagationResult:
    """The labelling S4.5.2 propagated to, and what it cost.

    Both, because the reconcile stage owes S4.5.6 a `label_prop_iterations` counter and
    the round count is not recoverable from the labelling: two partitions that look
    identical can have cost one round and eleven. :attr:`counters` is that payload, keyed
    by the name S4.5.6 declares, so ER-074 merges it rather than re-spelling it.
    """

    #: `record_key -> label`, where the label is the minimum `record_key` of the record's
    #: component. Complete over the node set that was passed in — including records with
    #: no incident edge, which label themselves — and in `record_key` order, so a caller
    #: iterating it writes rows in a total order (S4.5.4).
    labels: Mapping[str, str]

    #: Rounds executed, the last of which is the one that moved nothing and thereby
    #: proved the fixpoint. Never more than `clustering.max_iterations`, and never more
    #: than :func:`MAX_ITERATION_BOUND` of the node count.
    iterations: int

    @property
    def counters(self) -> dict[str, int]:
        """The S4.5.6 counter payload this stage contributes."""
        return {LABEL_PROP_ITERATIONS: self.iterations}


def _components(nodes: frozenset[str], pairs: Sequence[tuple[str, str]]) -> list[frozenset[str]]:
    """The connected components of the subgraph, by union-find.

    The FAILURE path's tool and nothing else's. It exists because S4.5.2 requires the
    non-convergence failure to log "the unconverged component's size and its minimum
    `record_key`", and at the moment the loop gives up the labelling is by definition not
    yet the answer to that question — a partial label group would report three records for
    a component of eight and send an operator looking in the wrong place.

    That it is cheap is not an argument for using it as the loop: S4.5.2 pins the
    incremental path to iterative label propagation in DuckDB SQL over data that already
    lives in the database, and INV-EQ's guard (T-INV-1) is a *reference* comparison, not a
    second production implementation. Here the edge list is already loaded, the run is
    over, and the only cost that matters is the diagnosis being right.
    """
    parent = {key: key for key in nodes}

    def root(key: str) -> str:
        while parent[key] != key:
            parent[key] = parent[parent[key]]
            key = parent[key]
        return key

    for rec_a_key, rec_b_key in pairs:
        roots = (root(rec_a_key), root(rec_b_key))
        # Hooked onto the smaller key rather than by rank: the root of a component is then
        # its minimum `record_key`, which is the label the loop was trying to reach.
        parent[max(roots)] = min(roots)
    grouped: dict[str, set[str]] = {}
    for key in nodes:
        grouped.setdefault(root(key), set()).add(key)
    return [frozenset(members) for members in grouped.values()]


def _non_convergence_message(
    labels: Mapping[str, str],
    nodes: frozenset[str],
    pairs: Sequence[tuple[str, str]],
    *,
    iterations: int,
    max_iterations: int,
) -> str:
    """S4.5.2's log line: which component did not settle, how big it is, its minimum key.

    The largest unsettled component, tiebroken by minimum `record_key` ASC so that two
    runs over the same subgraph name the same one (S4.5.4). A component is unsettled when
    any member's label is not its minimum `record_key`, which is the fixpoint the loop was
    driving at.
    """
    components = _components(nodes, pairs)
    unsettled = [group for group in components if any(labels[key] != min(group) for key in group)]
    # A round that moved a label cannot have left every component at its minimum, so
    # `unsettled` is never empty here; falling back to every component keeps a diagnosis
    # honest rather than raising a second exception while reporting the first.
    worst = min(unsettled or components, key=lambda group: (-len(group), min(group)))
    return (
        f"label propagation did not converge within clustering.max_iterations="
        f"{max_iterations} ({iterations} iteration(s) run over {len(nodes)} record(s), "
        f"{len(pairs)} edge(s)): {len(unsettled)} component(s) unsettled; the largest "
        f"holds {len(worst)} record(s) and its minimum record_key is {min(worst)!r} "
        f"(S4.5.2). No membership was written and no event was emitted"
    )


def _open_loop_relations(
    connection: duckdb.DuckDBPyConnection,
    nodes: frozenset[str],
    pairs: Sequence[tuple[str, str]],
) -> None:
    """Seed the `TEMP` state: every node labelled with itself, adjacency in both senses.

    Both senses of every pair, which is what makes the result independent of the
    orientation the caller happened to hold an edge in (S4.5.4, D1). The closed
    neighbourhood of `v` is symmetric by definition; canonicalising the input instead
    would leave `min` over a one-sided join answering a different question for `a` than
    for `b`.
    """
    connection.execute(
        f"CREATE OR REPLACE TEMP TABLE {_LABELS_RELATION} "
        f"(record_key VARCHAR NOT NULL, label VARCHAR NOT NULL)"
    )
    connection.executemany(
        f"INSERT INTO {_LABELS_RELATION} (record_key, label) VALUES (?, ?)",
        [[key, key] for key in sorted(nodes)],
    )
    connection.execute(
        f"CREATE OR REPLACE TEMP TABLE {_ADJACENCY_RELATION} "
        f"(node VARCHAR NOT NULL, neighbour VARCHAR NOT NULL)"
    )
    if pairs:
        connection.executemany(
            f"INSERT INTO {_ADJACENCY_RELATION} (node, neighbour) VALUES (?, ?)",
            [
                [end, other]
                for rec_a_key, rec_b_key in pairs
                for end, other in ((rec_a_key, rec_b_key), (rec_b_key, rec_a_key))
            ],
        )


def _close_loop_relations(connection: duckdb.DuckDBPyConnection) -> None:
    """Drop every `TEMP` relation the loop made, on success and on failure alike."""
    for relation in _LOOP_RELATIONS:
        connection.execute(f"DROP TABLE IF EXISTS {relation}")


def label_propagate(
    connection: duckdb.DuckDBPyConnection,
    nodes: Iterable[str],
    edges: Iterable[tuple[str, str]],
    *,
    max_iterations: int,
) -> LabelPropagationResult:
    """S4.5.2's incremental clustering: min-label propagation to a bounded fixpoint.

    `label(v) = min(record_key)` over the closed neighbourhood of `v`, propagated until
    nothing moves, with a pointer jump ending every round so the fixpoint is reached in
    :func:`MAX_ITERATION_BOUND` rounds rather than in one per hop.

    **Nothing is written.** Every round rewrites `TEMP` relations in the connection's
    in-memory database (S4.0b, M17), and they are dropped before this returns — the
    labelling is the only thing that leaves. That is what lets a stage commit membership in
    a single snapshot after clustering succeeds (S4.7), and it is why a non-convergence
    failure can guarantee "no snapshot committed, no events emitted": there is nothing to
    roll back.

    **The result does not depend on input order or on pair orientation.** Adjacency is
    stored in both senses and every step is a `min` over a set, so the same subgraph
    delivered in any order gives byte-identical labels — which is what D1/D2 (S4.5.4)
    require of the partition this feeds.

    Args:
        connection: the run connection (S4.0b). A bare `duckdb.connect()` is enough: this
            function reads no relation of the lake, and the unit layer passes exactly that.
        nodes: the affected node set — :attr:`AffectedSet.nodes`. A record with no incident
            edge is labelled with itself and MUST be passed in, or the singleton it becomes
            has no input (S4.5.3).
        edges: the pairs of the affected edge set, in either orientation —
            `edge.pair for edge in affected_edges(...)` in production. Probabilities are
            not read: the edge set is already cut at `auto_merge` (S4.5.1), and an edge in
            it is an edge.
        max_iterations: `clustering.max_iterations` from the validated config (S6, V12,
            default 50). Required, and never defaulted here: M26 puts the cap in the
            config document, so a literal in this module would be a second source of it.

    Returns:
        The labelling and the round count, as a :class:`LabelPropagationResult`.

    Raises:
        NonConvergenceError: the labelling had not settled after `max_iterations` rounds.
            S4.7 classifies it `non_convergence` (exit ``1``, not retryable) and the
            message carries the unconverged component's size and its minimum `record_key`.
        ValueError: `max_iterations` is below V12's floor of 1, or an edge names a record
            that is not in `nodes` — which would silently drop that record from the
            partition rather than fail.
    """
    if max_iterations < 1:
        raise ValueError(
            f"max_iterations={max_iterations} is below the S6.1 V12 floor of 1; the cap "
            f"comes from clustering.max_iterations on the validated config"
        )
    node_set = frozenset(nodes)
    # Self-pairs are dropped rather than rejected: a loop on `v` says nothing the closed
    # neighbourhood does not already say, and no canonical pair can be one (S5.0 requires
    # `rec_a_key < rec_b_key`).
    pairs = [(rec_a_key, rec_b_key) for rec_a_key, rec_b_key in edges if rec_a_key != rec_b_key]
    stray = sorted({key for pair in pairs for key in pair} - node_set)
    if stray:
        raise ValueError(
            f"{len(stray)} edge endpoint(s) are not in the node set and would be labelled "
            f"but never returned: {stray[:5]}. S4.5.1's edge set is the edges AMONG the "
            f"affected nodes, so an endpoint outside it is a caller error"
        )
    if not node_set:
        return LabelPropagationResult(labels={}, iterations=0)

    _open_loop_relations(connection, node_set, pairs)
    try:
        iterations = 0
        while True:
            iterations += 1
            connection.execute(_ROUND_SQL)
            row = connection.execute(_CHANGED_SQL).fetchone()
            assert row is not None, "count(*) returned no row"
            moved = int(row[0])
            if not moved:
                break
            connection.execute(_ADOPT_SQL)
            if iterations >= max_iterations:
                raise NonConvergenceError(
                    _non_convergence_message(
                        _labelling(connection),
                        node_set,
                        pairs,
                        iterations=iterations,
                        max_iterations=max_iterations,
                    )
                )
        labels = _labelling(connection)
    finally:
        _close_loop_relations(connection)

    # The other bound, and the other thing that has to be enforced: `max_iterations` is a
    # configured cap that a deployment can raise, while `ceil(log2 n) + 1` is a property of
    # the round. Converging *later* than the bound would mean the pointer jump had stopped
    # doubling the reach — a silent regression to one hop per round that no cap would
    # catch, since the default 50 covers a chain of 2^49 records.
    assert iterations <= MAX_ITERATION_BOUND(len(node_set)), (
        f"pointer jumping took {iterations} rounds over {len(node_set)} records, more than "
        f"the S4.5.2 bound of {MAX_ITERATION_BOUND(len(node_set))}"
    )
    return LabelPropagationResult(labels=labels, iterations=iterations)


def _labelling(connection: duckdb.DuckDBPyConnection) -> dict[str, str]:
    """The current `record_key -> label` mapping, in `record_key` order (S4.5.4)."""
    rows = connection.execute(
        f"SELECT record_key, label FROM {_LABELS_RELATION} ORDER BY record_key"
    ).fetchall()
    return {str(key): str(label) for key, label in rows}
