"""The affected NODE set of S4.5.1 — the formula, not a paraphrase of it.

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

**Shape.** A pure core — :func:`partners_of` and :func:`affected_nodes` take already
loaded rows and touch no connection — plus thin loaders that read the relations. The
core is what the unit layer exercises on a bare runner (S8.1), and it is also what makes
the S4.5.4 determinism argument checkable: given the same rows it returns the same sets.

**Scope, deliberately.** This module produces the node set. The affected EDGE set of
S4.5.1 — the `cut_edges` exclusion, the `int_std_records` restriction and the full
assertion adjustment of the clustering edge list — is ER-070's; label propagation is
ER-071's; reconciliation and the exit-`10` on an empty affected set are ER-074's. The
one piece of assertion adjustment done here is the minimum the *partner rule* is defined
over: minus active `never` pairs, plus active `always` pairs at `p = 1.0` (S4.4).
Nothing here writes, invalidates or deletes a row.
"""

from __future__ import annotations

from collections.abc import Iterable, Mapping, Sequence
from dataclasses import dataclass
from datetime import datetime
from typing import Any, Final

import duckdb

from er.entities.ids import record_key
from er.lake.model import SCHEMA_QUALIFIER
from er.review.assertions import ALWAYS, NEVER, Assertion
from er.review.queue import ENTITY, PAIR, RESOLVED_STATUSES

__all__ = [
    "ASSERTION_EDGE_PROBABILITY",
    "RECONCILE_STAGE",
    "SEED_ARMS",
    "SUCCEEDED",
    "AffectedSet",
    "SeedRecords",
    "affected_nodes",
    "current_membership",
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

#: The five arms of S4.5.1's `seed`, in the order the spec writes them. Exported so a
#: caller (and :class:`SeedRecords`) can enumerate them rather than restate them.
SEED_ARMS: Final[tuple[str, ...]] = (
    "batch",
    "content_hash_delta",
    "deletions",
    "assertion_delta",
    "review_delta",
)

_RAW_RECORDS: Final = f"{SCHEMA_QUALIFIER}.raw_records"
_INGEST_BATCHES: Final = f"{SCHEMA_QUALIFIER}.ingest_batches"
_ASSERTIONS: Final = f"{SCHEMA_QUALIFIER}.assertions"
_REVIEW_QUEUE: Final = f"{SCHEMA_QUALIFIER}.review_queue"
_ENTITY_MEMBERSHIP: Final = f"{SCHEMA_QUALIFIER}.entity_membership"
_RUNS: Final = f"{SCHEMA_QUALIFIER}.runs"
_RUN_STAGES: Final = f"{SCHEMA_QUALIFIER}.run_stages"


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


def _adjusted_edges(
    edges: Iterable[tuple[str, str, float]],
    assertions: Iterable[Assertion],
) -> dict[tuple[str, str], float]:
    """The scored edges with S4.4's adjustment applied: minus `never`, plus `always`.

    In memory and over the loaded result, exactly as S4.4 requires — the adjustment "is
    not a SQL clause and is not part of the `select`", and assertion edges are never
    persisted to `match_scores`.

    `always` first and `never` second, which is the order S4.4 fixes so that the two
    precedence readings cannot disagree: a pair carrying both would be removed either
    way. (S4.4 also rejects that pair at write time; the ordering here is what makes the
    claim true of any assertion set, including one a future writer produced.)

    Only ACTIVE assertions adjust anything. A retracted row is a historical record, not a
    constraint — it reaches the seed through the assertion arm and stops there.
    """
    adjusted = {(rec_a_key, rec_b_key): p for rec_a_key, rec_b_key, p in edges}
    active = [assertion for assertion in assertions if assertion.active]
    for assertion in active:
        if assertion.kind == ALWAYS:
            adjusted[assertion.pair] = ASSERTION_EDGE_PROBABILITY
    for assertion in active:
        if assertion.kind == NEVER:
            adjusted.pop(assertion.pair, None)
    return adjusted


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
