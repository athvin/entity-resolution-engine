---
name: er-server-control-plane
description: Explains this repo's Rust control plane (`er-server`) — how routes and the outer path normalizer are wired in axum, the bearer/API-key auth model and its 404-on-cross-org versus 403-on-role rules, the Postgres SKIP-LOCKED job queue whose two correctness constraints are enforced by the database rather than code, schema bootstrap from the generated baseline, the dispatcher's claim-run-mark loop, and how errors become `{detail}` bodies. Use when adding or reviewing an endpoint, handler, guard or job type, working on `pg.rs` or the dispatcher, debugging a 404/403/409 or a stuck job, or deciding where control-plane state belongs. For the typed OpenAPI contract and the frontend client see api-contract; for declaring a table or statement see entity-framework.
---

# The control plane

`er-server` is an axum service: the orgs / jobs / config / read-path HTTP
surface, plus a dispatcher that runs pipeline work. It is a port of a Python
FastAPI app, which explains several shapes that would otherwise look odd — most
visibly the `{detail}` error body the frontend BFF parses.

## Two backends, chosen by one field

`AppState.dsn` decides everything about persistence:

| `dsn` | Store | Used by |
|---|---|---|
| `None` | in-memory `JobStore` | the network-free HTTP unit tests, and local dev |
| `Some(_)` | the Postgres SKIP-LOCKED queue in `pg.rs` | production |

Postgres handlers **connect per request, with no pool**, mirroring the Python
`db.connect(dsn)` model. That is a deliberate simplification and it has a
consequence you must not break: `set_actor` declares `er.actor` for the
connection, which history triggers and the DDL ledger read. It is
**session-scoped**, and that is only safe because each request owns its
connection. If pooling ever arrives, that has to become transaction-scoped
first.

Because the unit tests run against the in-memory store, **a green
`cargo test --workspace` says nothing about the Postgres paths.** They self-skip
without `ER_TEST_PG_DSN`. See the rust-conventions skill's gates reference.

## Routing: one flat chain and an outer normalizer

Routes are a single flat `Router::new()` chain — roughly sixty `.route()` calls,
no `.nest()`. `/healthz` and a few globals, then everything tenant-scoped under
`/v1/orgs/{org}/…`.

The `:action` path normalizer is installed as an **outer tower service**
(`Router::new().fallback_service(…)`), deliberately *not* as a
`Router::layer`. A layer runs after routing has already happened, so it could
not influence which route matched. If you touch this, keep it outside the
router.

## Auth: the lattice, and which status code means what

Every `/v1` route sits behind a bearer boundary that resolves either the static
operator token or an API key:

- Key format `erk_{key_id}_{secret}`. **SHA-256-hashed at rest, never argon2** —
  these are high-entropy generated secrets, not user passwords, so a slow KDF
  buys nothing and costs latency on every request.
- The secret may itself contain `_`, so parsing must not naively split on every
  separator.

Role lattice: `viewer < steward < admin < operator`. `guard(principal, org,
minimum)` enforces it, and the two failure codes differ on purpose:

| Situation | Status |
|---|---|
| principal's org differs from the requested org | **404** |
| org matches, role insufficient | **403** |

**404, not 403, for a cross-org request.** A 403 would confirm the org exists,
which is an enumeration oracle for a tenant whose existence should not leak.
The operator token is cross-org and bypasses the scope check.

Child tables carry `org_id`, so a scope-safe lookup is a single query rather
than fetch-then-check.

## The queue's correctness is Postgres's job

Two constraints are enforced **by the database, not by code**:

1. At most one **active** job per org — single-writer per tenant.
2. At most one job per `(org, idempotency_key)`.

So `claim` takes the highest-priority queued job whose org has no active job,
`FOR UPDATE SKIP LOCKED`. When it loses that race the one-active-per-org
constraint surfaces as a unique violation, and that is mapped to `None` — "no
job for me right now" — **never to an error**. A lost race is normal operation
for a concurrent dispatcher; treating it as a failure would make the logs
useless and the alerting wrong.

`enqueue` returns the job an identical idempotency key already created, so a
retried submit is not a duplicate run.

Job states walk `queued → running → {succeeded, failed, canceled}`. `Job` is a
re-export of the **generated** `er_entities::job::Job`, so the queue, the
handlers and the dispatcher share exactly one row definition — the queue's SQL
and row mapping are generated from `entities/job/`, not hand-written.

## Schema bootstrap

`ensure_schema` takes `pg_advisory_xact_lock(hashtext('erserver.ensure_schema'),
0)`, sets `er.ddl_log_skip` for the transaction, replays the generated
`POSTGRES_BASELINE` — every `public.*` table's DDL in FK-dependency order, built
from the entity catalog — and then runs `er_entity_diff::reconcile_postgres`
against the compiled-in catalog: the `ADD COLUMN` the baseline's `IF NOT
EXISTS` cannot do, each statement ledgered in `public.schema_migrations`. A
breaking difference is `SchemaError::Reconcile` (`ERR_SCHEMA_BREAKING`, every
difference listed) and the boot stops; `main.rs` prints the chain and exits.

- **No schema is hand-written here.** A new control-plane table comes from an
  `entity.yaml`; see the entity-framework skill.
- The advisory lock makes concurrent boots safe.
- `er.ddl_log_skip` stops an idempotent boot replay from filling the DDL ledger
  with no-op events; the reconciler's statements are real migrations and are
  NOT skipped, so they appear in `ddl_log` as well as the ledger.
- `er-server migrate` is the same thing as a subcommand: the web-api pod's init
  container, and the one-off master-DSN run the event triggers need.
- `er-server migrate-tenants [--timeout-secs N]` is the lake half
  (`schema.rs`): one `lake_migrate` job per active org, a wait, a per-tenant
  report, non-zero exit on `breaking`/`error`/`timeout`. The chart runs it as
  a post-upgrade hook and a daily CronJob.

`ensure_event_triggers` is **best-effort by design**: `CREATE EVENT TRIGGER`
needs superuser/rds_superuser, which CI and dev have but the application role
deliberately does not. It returns `Ok(false)` on insufficient privilege and the
caller logs and boots on. Production installs them through the deploy migrate
job with the master DSN. Do not "fix" this into a hard failure.

## The dispatcher

`dispatcher::run_loop` polls; `dispatch_once` does claim → mark running →
**reconcile the org's lake** (`schema::reconcile_lake_env`, recorded in
`public.tenant_schema_states` whatever the outcome; a breaking lake fails the
job with `ERR_SCHEMA_BREAKING` before any stage runs) → open the org's lake →
run the matching `er-cli` operation **in-process** → mark the outcome → emit
the `job.<state>` webhook. `provision` is the one kind that skips the
pre-step and reconciles the lake it creates instead; `lake_migrate` is the
kind whose whole work is the pre-step. There are no per-run Kubernetes jobs.
`dispatch_once_with` takes the provisioning profile explicitly for the
Postgres-tier tests.

- Idle backoff doubles from 500ms to a 5s ceiling.
- A Postgres failure returns `Err` and is fatal to the caller: the standalone
  binary exits non-zero so Kubernetes restarts it; in in-process mode the
  spawned task logs and the API keeps serving.
- Termination requests a drain of the in-flight job, then a clean exit.

**A job whose required inputs are absent is FAILED with a clear reason, never
silently skipped.** Inputs resolve from the org row and its `env` — lake catalog
and data path, config path, deliveries per configured source, model and TF
paths. A silent skip would make a misconfigured tenant look healthy.

Handler conventions, the read path, and what is deliberately not ported yet:
[references/handlers-and-state.md](references/handlers-and-state.md).
Queue, schema and actor details: [references/postgres.md](references/postgres.md).

## Errors become `{detail}` strings

There is **no central `ApiError` or `IntoResponse` impl**. Handlers return
`impl IntoResponse` and build the body inline:

```rust
(StatusCode::BAD_REQUEST, Json(json!({ "detail": msg })))
```

That shape is what the frontend BFF parses — changing it is a frontend-visible
break.

Two rules when producing `msg`:

- **Flatten the chain.** Workspace error types keep their cause out of
  `Display`, so `e.to_string()` gives only the outermost message. Use
  `errfmt::chain(&e)` for a `thiserror` type, `format!("{e:#}")` for anyhow.
- **Internal errors must not reach clients.** A `format!("db: {e}")` body leaks
  schema and driver detail. Return a generic `detail` and log the rest.

## The hard boundary on lake SQL

`er-server/src` must contain **no `lake.main.<relation>` literal and no
`query_strings` caller**. This is a posterity gate (`freeform_sql.rs`), not a
convention. Its reads come only from generated `er_entities::` statements and
the golden merge-plan generator.

So if a control-plane feature needs lake data, the work is to declare a
statement in `entities/`, not to assemble SQL in a handler. This is the rule
most likely to block an otherwise-finished change — check it before you start.
