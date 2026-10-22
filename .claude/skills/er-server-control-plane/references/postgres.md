# The Postgres layer

## Contents

- Shape of the module
- Connecting, and the actor
- Schema bootstrap
- The queue functions
- Why a lost claim is not an error
- API keys
- Timestamps on the wire
- Testing this layer

## Shape of the module

`pg.rs` is a flat module of free `async fn`s over `tokio_postgres::Client` — no
repository struct, no trait. It is a port of the Python `db.py` / `queue.py` /
`auth.py` / `configsvc.py`, and the flat shape is inherited from that.

Everything it stores is generated from the entity catalog. The queue functions
in particular are thin wrappers over `er_entities::job::*`: the SQL and the row
mapping come from `entities/job/`, so a column added to the job entity flows
through without a hand-edit here.

## Connecting, and the actor

| Function | Use |
|---|---|
| `connect(dsn)` | `tokio_postgres::connect` with `er_pg_tls::make_tls()`, spawning the driver task |
| `set_actor(client, actor)` | `SELECT set_config('er.actor', $1, false)` |
| `connect_pg_as(dsn, actor)` | both, together |

TLS goes through `er-pg-tls`, which gives `sslmode=require` semantics —
encrypt, do not verify — because the RDS CA is not in system roots.

**The actor is session-scoped.** The `false` third argument to `set_config`
means "not local to the transaction", so it persists for the connection.
History triggers and the DDL ledger read `er.actor` to record who made a
change; absent it, they fall back to the session user.

This is only correct because handlers connect per request and the dispatcher
owns one connection. **If a pool is ever introduced, `er.actor` must become
transaction-scoped first**, or one request's actor will be attributed to
another's writes. That is a silent data-integrity bug in an audit trail, which
is the worst place to have one.

## Schema bootstrap

`ensure_schema`:

1. `SELECT pg_advisory_xact_lock(hashtext('erserver.ensure_schema'), 0)` — so
   concurrent boots serialize rather than racing on `CREATE TABLE`.
2. `SET LOCAL er.ddl_log_skip = 'on'` — an idempotent replay should not fill the
   DDL ledger with no-op events.
3. Replay `er_entities::schema::POSTGRES_BASELINE`: every `public.*` table's
   DDL in FK-dependency order, generated from the entity catalog.

`ensure_event_triggers` installs the DDL-audit triggers **best-effort**,
returning `Ok(false)` when the role lacks privilege. See the SKILL body for why
that is correct rather than a bug.

## The queue functions

| Function | Behaviour worth knowing |
|---|---|
| `enqueue` | returns the job an identical `(org, idempotency_key)` already created |
| `claim` | highest-priority `queued` job whose org has no active job, `FOR UPDATE SKIP LOCKED` |
| `mark` | moves a job to a terminal or running state |
| `fail_or_retry` | the failure path, distinguishing a retry from a terminal failure |
| `resume_job` | back to `queued`, `not_before = now()`, `finished_at` cleared, with a `resume_run_id` |
| `get_job`, `list_jobs`, `list_jobs_all` | scoped and cross-org reads |
| `merge_org_env` | `UPDATE orgs SET env = env || $2` — a shallow merge, not a replace |
| `ensure_org`, `upsert_org_byo`, `org_exists`, `list_orgs`, `get_org` | org lifecycle |
| `issue_key`, `lookup_key`, `revoke_key` | API keys |

`resume_job` clearing `finished_at` and setting `not_before` matters: a resumed
job must look queued to `claim`, and runs are deterministic over their pinned
inputs, so a resume reproduces rather than approximating.

`merge_org_env` being a `||` merge means a caller updating one key does not
have to read-modify-write the whole object — but it also means **you cannot
remove a key this way**. Removing requires an explicit write.

## Why a lost claim is not an error

`claim` races with every other dispatcher. The database enforces at most one
active job per org, so when two dispatchers pick the same org's job, one of
them violates that constraint.

That unique violation is mapped to **`Ok(None)`** — "nothing for me right now" —
not to `Err`. The reasons:

- It is expected, routine behaviour at any concurrency above one.
- Returning `Err` from `claim` is fatal to the dispatcher loop by design (a
  Postgres failure should restart the process), so misclassifying a lost race
  would crash-loop the dispatcher under normal load.
- It would also bury real Postgres failures in noise.

If you add a queue operation that can lose a benign race, follow the same
pattern: distinguish "someone else got there first" from "the database is
broken".

## API keys

Minted as `erk_{key_id}_{secret}` and stored as a **SHA-256 hex digest** — the
plaintext is returned once at mint time and never recoverable.

`lookup_key` resolves a presented key to its org and role. The hash choice is
deliberate: these are high-entropy generated secrets, so argon2's work factor
would add per-request latency without adding meaningful resistance. User
passwords are a different problem with a different answer.

Revocation is scoped by org, so one tenant's admin cannot revoke another's key.

## Timestamps on the wire

Timestamps render as **RFC 3339 microseconds UTC** via a local `ts` helper, to
match the entity wire contract (OpenAPI `date-time`). The projection structs
that carry text timestamps use it rather than formatting ad hoc, so the API and
the generated TypeScript types agree. Do not introduce a second format.

## Testing this layer

Both suites are gated behind `ER_TEST_PG_DSN` and **self-skip** without it:

| Suite | Exercises |
|---|---|
| `tests/pg_e2e.rs` | the queue functions directly |
| `tests/pg_http.rs` | the same queue through the HTTP surface |

Plus a large `pg_*.rs` family covering auth, config, publish, dispatch, drain,
orgs, golden reads, history, the DDL log, schedules, schema coverage, steward
flows, unmerge and webhooks.

**Every `pg_*.rs` file must contain exactly one `#[tokio::test]`** with
sequential phases — enforced by `pg_serial.rs`, because the tests in a binary
run concurrently and share one schema. Add a phase to the existing test rather
than a second test function. The rust-conventions skill's gates reference
explains why this failure mode is especially nasty (it passes locally and in
the default CI job, and only fails where a database exists).

The HTTP tests drive `app()` through `tower::ServiceExt::oneshot` — no
listening socket — so they are fast and need no port allocation.
