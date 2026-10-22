# The named invariants

## Contents

- Why they are named
- INV-SCORE — a score is a pure function
- INV-EQ — when incremental equals full
- INV-PERM — an unchanged group keeps its id
- CONTRADICTION-1 — contradictory assertions fail the run
- Where each one is tested

## Why they are named

`DesignDoc.md` is the normative specification. It defines each invariant
**once**, at one section, and cites it by name everywhere else. The names are
not shorthand for a vibe — they are precise claims with preconditions, and
several have acceptance tests whose whole job is to prove the precondition set
is real rather than decorative.

Quote them from the spec. Do not paraphrase from memory, and do not widen one:
"incremental equals full" without its five preconditions is a claim the system
does not make.

## INV-SCORE — a score is a pure function

> **INV-SCORE** — `match_probability` is a pure function of `(model_version,
> tf_snapshot_id, rec_a_key, rec_b_key, rec_a_content_hash, rec_b_content_hash)`.

Spec section: S4.3 (`DesignDoc.md`, search `INV-SCORE`).

What it rules out, in practice:

- Term frequency recomputed from whatever corpus a run happened to see. This is
  the `run_predict` vs `run_predict_with_tf` distinction — see the SKILL body.
  The paired requirement is **D4**: TF is registered *frozen* from `tf_lookup`.
- Scores that drift with batch composition, row order, or thread count.
- Editing comparison levels or m/u values under an existing `model_version`. A
  parameter change is a new `model_version`, always.

A corollary worth stating because it is load-bearing for incremental work: a
pair's score can be cached and reused across runs, and a rescore is only
required when one of those six inputs changes.

## INV-EQ — when incremental equals full

> **INV-EQ** — for a pinned `model_version` with a pinned `tf_snapshot_id`, an
> append-only corpus (no `content_hash` changes to previously-ingested records,
> no deletions), an unchanged `std_version` and `config_hash`, and an identical
> active assertion set, **incremental reconcile produces the same set-partition
> of current members as full reconcile.**

Spec section: S4.5.6.

The five preconditions, enumerated so they can be checked one at a time:

1. `model_version` pinned
2. `tf_snapshot_id` pinned
3. corpus append-only — no content-hash changes to already-ingested records, no
   deletions
4. `std_version` and `config_hash` unchanged
5. the active assertion set identical

**It is a set-partition claim.** Entity ids may differ between a from-scratch
universe and an incrementally-maintained one; they are compared as partitions
of current members, with ids deliberately out of scope. Asserting id equality
produces a failure that is not a defect.

**It is conditional, and the conditions are tested as conditions.** The spec is
explicit that this is not an unqualified equivalence claim: where a
precondition is violated, the divergence must be *exhibited* by a test,
bounded, and repaired by the periodic correction pass — not asserted away. So
there are two acceptance tests, one for the invariant and one for its
falsifiability.

**The two loss vectors**, named in S4.5.6 and the only things `er correct`
repairs:

- **Incremental candidate generation** — an incremental pass can never pair two
  records that were both already in the corpus, because neither is new. A full
  pass can.
- **Corpus-dependent term frequency** — TF computed from a growing corpus moves,
  so a snapshot taken earlier drifts from the corpus it is scoring.

`er correct` regenerates candidate pairs over the whole corpus, re-materializes
TF under a new `tf_snapshot_id`, re-scores at the **active** `model_version`,
re-clusters the full assertion-adjusted edge set, and re-assembles. Because it
never trains, `model_version` and every fitted value are unchanged across it,
so INV-PERM still governs the ids it rewrites.

## INV-PERM — an unchanged group keeps its id

> **INV-PERM** — let `P_old` be the current membership partition of
> `affected_nodes` and `P_new` the clustering output over the same nodes. For
> every group in `P_new` that is **set-equal** to a group in `P_old`, every
> member retains its existing `entity_id`, no ULID is minted, and no event is
> emitted. Otherwise, build the overlap matrix `|g_new ∩ g_old|` between `P_new`
> and `P_old` and apply: …

Spec section: S4.5 (the continuation defines survivor selection for merges and
the deterministic ordering for splits).

Consequences:

- Identity is **stable under re-running**. A no-op reconcile emits no events,
  which is also why an event stream is meaningful as a change log.
- A merge picks a survivor and leaves redirects for the losing entities; a
  split keeps the old id on one fragment and mints ids for the rest under a
  deterministic ordering, so the outcome is reproducible rather than
  insertion-order dependent.
- An identifier is **not** a similarity score. Two records sharing an
  `entity_id` means a clustering decision was recorded, nothing more.

## CONTRADICTION-1 — contradictory assertions fail the run

> **CONTRADICTION-1** — before clustering, compute the connected components of
> the `active` `always` edges (the *always-closure*). If any `active` `never`
> pair has both endpoints inside one component, the run **FAILS**: exit `1`, no
> snapshot committed by the reconcile stage, no events emitted, and the
> offending `assertion_id`s plus the closure component are named in
> `run_stages.error_detail`.

Spec section: S4.5.

The important part is the *closure*: the check is not pairwise. An `always`
chain A–B, B–C puts A and C in one component, so a `never` on A–C is a
contradiction even though no single assertion contradicts another directly. A
`never` constraint is likewise enforced across transitive connections during
reconciliation, not only on the directly scored pair.

Failure is loud and total by design — exit 1, nothing committed. A
contradictory assertion set is a data-entry problem a steward must resolve, and
silently dropping one of the two assertions would hide it.

## Where each one is tested

The specification's own test-ID table (`DesignDoc.md`, the `T-…` rows) maps
each invariant to its acceptance test. Note the spec still lists the retired
Python paths (`tests/integration/test_incremental.py`) — the implementations
now live in the Rust workspace, as per-crate integration tests under
`rust/crates/<crate>/tests/`. Use the test *names* from the spec and find them
with grep; do not trust the paths.

| Invariant | Acceptance test, by intent |
|---|---|
| INV-SCORE / D4 | the frozen-TF match goldens: `base_10` scores reproduce regardless of batch membership |
| INV-EQ | the incremental-equals-full pair: one test for the invariant, one that violates exactly one precondition, shows the bounded divergence, then repairs it with `er correct` |
| INV-PERM | the reconcile scenario goldens (assertion, merge, split, supersession, deletion) |
| CONTRADICTION-1 | the contradiction golden in `er-reconcile` |
