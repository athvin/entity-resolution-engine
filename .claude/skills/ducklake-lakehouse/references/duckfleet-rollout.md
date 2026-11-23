# DuckFleet API workflow and rollout decision — #1134

Decision: **NO-GO for a production remote-execution cohort.** Embedded execution
remains supported and is the default. No production configuration, customer
cohort, pricing or launch dependency changes. The qualified remote envelope stays
limited to generated reads and account golden generation on DuckDB 2.0.1.

## Customer workflow result

The existing launch-proof public-job workflow ran embedded stages and the remote
account golden task through real `exec-task` children, PostgreSQL, object storage
and DuckFleet. It produced identical golden output and lineage on both backends,
preserved the three matching scores, and correlated ten remote statements. This
is evidence for the account golden operation, not the whole configure → ingest →
run → retrieve workflow.

A customer-scoped API key cannot complete that whole workflow with remote policy
installed: ingest, standardize, match, reconcile and training now refuse before a
lake opens. This is an intentional safe outcome from #1170, not a silent skip or
an embedded fallback. Generated reads remain available remotely. Consequently
the full customer-scoped, both-backend acceptance gate is **not passed**, and no
production enrollment is permitted.

## Measured fixtures and pass thresholds

The small correctness fixture is four account golden records, sixteen lineage
rows and three matching scores. The generated-read fixture checks exact float
bits, typed nulls, rollback, cancellation and two matching local/remote snapshot
IDs. The large representative fixture remains the existing 50,000-record scale
golden; it has not been run through the remote pipeline and is not evidence of
remote throughput.

Before any reconsideration, a candidate must record these thresholds and compare
both executors on identical data and settings:

- **Correctness:** exact golden records, lineage, types and nulls; zero silent
  skips; every unsupported operation has its documented refusal.
- **Warm/cold latency:** at least 30 warm runs and 5 cold runs per backend;
  remote p95 no more than 20% slower than embedded, with every startup,
  orchestration and queue interval reported separately.
- **Throughput:** at least three repetitions of the 50,000-record fixture;
  completed rows/second no more than 10% below embedded, with failed and retried
  work excluded from successful-run throughput but included in cost.
- **Resources and economics:** report CPU-seconds, GiB-seconds, persistent idle
  capacity, isolated startup, orchestration, storage/network and failure/retry
  usage. Estimated rate-card cost per completed run must include all of those
  inputs and must not be represented as customer billing or a reconciled cloud
  invoice. Require no more than 20% regression against embedded.

The current evidence does not establish those latency, throughput or economic
thresholds. No numbers are extrapolated from the four-record fixture.

## Deployment and retention boundaries

Dataset provisioning, additive schema reconciliation and catalog-version pinning
remain embedded operations under remote policy. Automatic catalog migration stays
disabled; application schema changes use the existing additive reconciler.
Snapshot/keyset reads proved snapshot ID 2 on both executors, but deploy-spanning
catalog upgrades and cursor survival under production retention have not been
qualified. Unsupported paths therefore cannot be enabled to satisfy this gate.

The existing read/golden evidence covers cancellation, writer exclusion, explicit
close and zero leaked sessions. Lost COMMIT acknowledgement reconciles exactly
one publication receipt as `committed`; an unsent COMMIT has no receipt and
reconciles `rolled_back`. This does not prove every pipeline transaction or an
unknown external effect.

## Reconsideration and rollback

Reconsider only with a new qualification change that supplies the missing
customer-scoped workflow, deployment/retention proof, large-fixture thresholds,
AWS identity and production-secret checks, monitoring and named operational
ownership. If those ever pass, enroll only the measured cohort and rehearse
operator removal of `ER_DUCKFLEET_TASK_POLICY` plus reconciliation of active
attempts before expansion.

For the current qualified remote paths, removing the policy stops new remote
runs; embedded execution needs no migration or data rewrite. Existing uncertain
writes retain their attempt fence and receipt recovery and are never replayed
merely to disable the backend.

## CI comparison

The manually requested frontend run `45242678850` passed. The manually requested
Rust run `45242674359` passed the required backend suites and failed only the
optional 50,000-record golden comparison: both golden tests reported 32
`match_weight` and 32 `match_probability` mismatches. A comparison run of
unmodified `main` (`45249333886`, commit `ad38be23e193e35d7e366d6e590255b3d1631f62`)
reported the same two failures and mismatch counts. This is pre-existing scale
fixture drift, not remote-execution evidence and not a reason to change engine
behavior in this documentation-only rollout decision.
