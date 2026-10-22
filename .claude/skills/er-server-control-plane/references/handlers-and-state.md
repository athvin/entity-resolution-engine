# Handlers, state, and the read path

## Contents

- `AppState`, field by field
- Handler conventions
- The scope-safe lookup
- The read path: snapshots and cursors
- Config publish is an analysis step
- Request-body limits
- What is deliberately not ported

## `AppState`, field by field

| Field | Role |
|---|---|
| `operator_token` | the cross-org admin bearer. **Empty disables operator auth** |
| `store` | the in-memory `JobStore`, used when there is no DSN |
| `dsn` | `Some` selects the Postgres queue; `None` the in-memory store |
| `mail` | outbound email; defaults to `Mailer::off` |
| `max_import_bytes` | the import body cap, defaulting to `DEFAULT_MAX_IMPORT_BYTES` |

Two defaults are load-bearing for testability:

- `mail` defaulting to off is why the network-free unit tests can build a state
  without configuring a backend — `POST /v1/email` then answers 503, exactly as
  an unconfigured environment does. The test and production behaviours agree
  rather than diverging behind a mock.
- `operator_token` being empty disabling operator auth means a test can
  construct a state with no cross-org escape hatch at all.

Note what is **not** here: there is no connection pool, because handlers
connect per request.

## Handler conventions

- Return `impl IntoResponse`; build `(StatusCode, Json(json!({"detail": …})))`
  inline. There is no central error type — see the SKILL body.
- Take `State<AppState>` and the path/query extractors; resolve the principal
  and call `guard` before doing anything else.
- Request and config DTOs carry `#[serde(deny_unknown_fields)]`. This is the
  mass-assignment defence: without it, a client can send fields the struct does
  not declare and they are silently ignored, which hides typos and makes a
  future field addition a silent behaviour change. Add it to every new DTO.

## The scope-safe lookup

`org_or_404` is the helper to use rather than hand-rolling a fetch-then-check.
It resolves the org and returns the standard scope-safe 404 when absent, which
keeps the "absent" and "not yours" cases indistinguishable from outside.

Child tables carry the org's UUIDv7 as `org_id`, so a scoped query filters
directly instead of joining up to the org first.

The 409 case is separate: a lifecycle transition (suspend/resume) returns 404
when the org is absent and **409 when it is not in the expected state**. Those
are different answers to different questions and should not be collapsed.

## The read path: snapshots and cursors

`read.rs` resolves an org to its lake from `orgs.env`, opens it **read-only**,
pins a snapshot (DuckLake time travel), and queries `lake.main.*`.

A page is `{ items, snapshot, next_cursor }`. The cursor is an **opaque
base64url** of `{s: snapshot, k: last_key}` — the pinned snapshot plus the last
key emitted — so:

- Paging is **stable against a moving lake**. Page 2 reads the same snapshot
  page 1 did, so rows cannot shift between pages while a run is writing.
- It is keyset paging, not offset paging, so cost does not grow with depth.

The coupling to remember: because a cursor holds a snapshot, **snapshot expiry
must outlive the longest-lived cursor.** Expiring history out from under a
paginated read breaks it, and the symptom looks like an intermittent reader
bug. See the ducklake-lakehouse skill.

Treat the cursor as opaque in clients. Its contents are an implementation
detail, and the base64 is not a security boundary — it is a "do not parse this"
signal.

## Config publish is an analysis step

`config_check.rs` runs before a config version goes live: it verifies that the
blocking and comparison definitions only reference columns the standardized
records table actually has for the config's entity type. A mistyped or
non-existent column is refused **at publish** rather than failing a tenant's
first run.

Two things to know about its scope:

- A blocking rule's `expr` is a **raw scalar SQL expression** over standardized
  columns, parsed to extract column references.
- The check is about *column references*. It is not a general safety analysis of
  the expression, so publish remains an authenticated code-execution surface
  gated by role — the security skill treats it that way, and so should you.

## Request-body limits

The import cap is enforced **twice**: as the route's body limit and again while
the multipart part streams. That is not redundancy — a declared body limit can
be evaded by a chunked upload that never declares a length, so the streaming
check is the one that actually bounds memory.

It is configurable so an operator can tighten it, with a default in the code.

## What is deliberately not ported

The crate documents its own gaps, and they are worth checking before assuming a
surface exists:

- The steward **write** surface — assertion create/delete, review resolve,
  unmerge, job resume — and merge-plans. These need the review / assertion /
  run-tracking data-plane stages that were removed with the Python
  implementation.
- OIDC BFF auth and the Kubernetes launcher, which are infrastructure-bound.
- The mailer is live but inert until an environment configures a backend.

When a frontend feature appears to need one of these, the work is backend-first.
Do not stub it in the BFF.
