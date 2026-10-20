# Business logic

The abuse that doesn't look like "hacking" — using the product's own verbs to corrupt resolution
results or exhaust the service. For an entity-resolution engine these are often higher-impact than a
classic web bug, because they poison the data the customer trusts.

## Contents
- Assertion abuse (the strongest lever)
- Threshold manipulation
- Request-lane DoS
- Job queue-stuffing
- Config-publish panics
- Unmerge invariants (what's already good)
- How to prove / how to lock

## Assertion abuse (the strongest lever)

`POST /v1/orgs/{org}/assertions` (`er-server/src/lib.rs` `add_assertion`, `Role::Steward`) accepts
arbitrary `a` / `b` record keys with **no existence check**, no check they belong to the org's corpus,
no volume cap, no 4-eyes. An `always` assertion **forces two records to merge** (fusing unrelated
identities into one golden record); a `never` forces a cut. Assertions survive re-ingest, so the
damage is durable. This is the single strongest business-logic lever in the product.

Fix: validate `a`/`b` exist in the org's current lake snapshot before staging; consider a per-window
volume cap. Test (#9): extend `er-server/tests/pg_steward.rs` (the assertion-staging harness) —
nonexistent key → 422, not staged.

## Threshold manipulation

`thresholds.auto_merge` / `review_low` are plain `f64` with **no range validation** in `er-config`.
Tier-A publish is admin-reachable; setting `auto_merge: 0.0` auto-merges the entire corpus into mush.
Fix: validate `0.0 <= review_low <= auto_merge <= 1.0` (and whatever the domain requires) at config
load. Test: a `config_check.rs` unit case (part of #10).

## Request-lane DoS

- `GET /v1/orgs/{org}/match-preview` (`lib.rs` `match_preview`, Steward) runs the **full blocking +
  scoring pipeline synchronously** on a `spawn_blocking` thread, over the whole corpus. `limit` is
  clamped 1..200 but the *blocking cardinality* is not bounded by it — combine with a pathological
  config blocking rule for quadratic blowup.
- **Connect-per-request Postgres** (`connect_pg`, no pool) — a connection-exhaustion lever under
  concurrency.

Neither has a crisp functional assertion. Settle for the structural gate (#11): handlers use a pooled
client; preview work is bounded. Mark **partial coverage** in the report — this is the one class where
"lock it with a test" is genuinely awkward, and honesty about that is the point.

## Job queue-stuffing

`POST /v1/orgs/{org}/jobs` (`lib.rs`, Steward) validates `kind` only as non-empty — no vocabulary
check (contrast `SCHEDULABLE_KINDS`, which *is* enforced for schedules). Unknown kinds fail later in the
dispatcher, so it's a queue/table-growth lever, not RCE. `pg::claim` enforces one-active-per-org,
which bounds execution but not insertion. Fix: validate `kind` against the known vocabulary at
submission. Test: extend a jobs pg test (e.g. alongside `pg_dispatch.rs`).

## Config-publish panics

`er-standardize/src/lib.rs` indexes `spec.columns["given_name"]` etc. — **panics** if a published
config omits a canonical key, crashing the pipeline (DoS via config publish). Fix: validate presence
at config load (return `Err`, never index-panic). Test (#10): a `config_check.rs` unit + a `stage.rs`
plan-vector case asserting a config missing a mapped column yields an error, not a panic.

## Unmerge invariants (what's already good — keep it)

`POST /v1/orgs/{org}/golden-records/{entity_id}/unmerge` (`lib.rs`, Steward) has solid invariants:
1..=100 records, members must be real, cannot unmerge every member, and a contradiction-1 precheck
against the active ledger before staging. `POST /mass-update` is also well-defended (bounded 1..=200,
`OVERRIDABLE_ATTRS` check, re-validated against an 11-entry allowlist in
`er_cli::apply_staged_action`). Lock these as *positive* invariants where cheap; don't rewrite them.

## How to prove / how to lock

| Claim | PoC | Test |
|---|---|---|
| Forced merge via fake keys | `POST /assertions` with nonexistent `a`/`b` → staged | `pg_steward.rs` (#9) |
| Corpus-wide auto-merge | publish `auto_merge: 0.0` → accepted | `config_check.rs` unit (#10) |
| Publish panics the pipeline | config missing `given_name` mapping → panic | `stage.rs` plan-vector (#10) |
| Preview/DoS unbounded | handler opens a per-request connection; preview unbounded | static gate (#11), partial |
| Job kind unchecked | submit `kind: "nonsense"` → accepted | jobs pg test |
