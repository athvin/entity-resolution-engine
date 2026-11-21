# DuckFleet generated-read pilot

Issue #1130 adds an execution seam for the existing person/account golden-page
queries. The entity generator emits `*_with_executor` beside the existing
embedded functions; the SQL, positional binds and typed snapshot splice still
come from `entities/`. Existing embedded signatures remain available. The
server's page shaper uses the shared path and retains the existing Arrow schema,
search escaping, keyset cursor, page limit and response headers.

`er-entities::execution::Executor` accepts owned typed parameters and returns
Arrow batches. `er-duckfleet::execution::Session` owns either an embedded
connection or one remote HTTP session. It exposes open, a cloneable cancellation
control and explicit close. A session retains temporary state and transaction
scope. Transport loss carries a request UUID, marks the session unusable and
never triggers replay or replacement. Errors contain fixed classifications,
never upstream bodies, SQL, parameter values or credentials. Cancellation and
timeout do not establish that a write failed to commit.

The HTTP adapter refuses redirects, disables transport retries, bounds control
responses and Arrow results, and uses the upstream statement limit. Unsupported
list parameters and non-finite doubles fail before sending a request. Scalar
strings, booleans, signed integers, finite doubles and nulls bind as JSON values;
SQL casts carry decimal/date/timestamp semantics. These are transport limits,
not changes to the public API. Explicit close reports cleanup failure and can
be retried. Drop only attempts bounded cleanup; it is not proof of release.

## Operator selection

By default all server reads use the embedded backend. The process-wide pilot
switch applies only to the golden-record page route:

- `ER_DUCKFLEET_READ_REGISTRY`: absolute path to the operator-owned registry from
  #1129.
- `ER_DUCKFLEET_CREDENTIALS_DIR`: absolute projected-credential mount path.

Set both or neither. Partial/invalid configuration fails closed. After the
existing role guard and org-scoped dataset lookup, each operation reloads the
registry and resolves the current active generation for the immutable org and
dataset IDs. Missing, expired or revoked bindings return service-unavailable;
there is no local fallback. Endpoints and credentials never come from customer
requests or dataset configuration. A running session retains its original
capability; upstream revocation and drain complete the operator lifecycle.

The switch is deliberately not configured in any environment by this ticket.
A dedicated deployment must contain the correct dataset catalog before reads
are enabled. Other read routes and task execution remain embedded until their
respective work is implemented. Production adoption still depends on the
engine/catalog compatibility and full-workflow qualification in #1134.

## Evidence and reproduction

`make check-duckfleet-reads` builds a pinned DuckFleet revision, creates a unique
Compose project and invokes the actual Rust adapter and generated golden query.
It checks typed values and nulls, filtered/keyset pages at a pinned snapshot,
visibility after a later update, exact float bits, temporary state, rollback,
SQL-error recovery, cancellation and explicit cleanup. It verifies that the
worker reports zero open sessions and active queries, then removes its own
containers and volumes. No existing developer fleet is touched.

The loopback-only constructor used by this harness is compiled solely with the
`disposable-proof` feature; product builds do not expose an endpoint override.
The JSON report is retained in
`duckfleet-read-evidence.json`. Compose evidence
establishes this read slice, not live AWS isolation or production equivalence.

The baseline compatibility probe identified JSON float decoding that changed
`0.0012033694344163659` by one bit. Upstream DuckFleet #116 / PR #117 enables
serde_json's round-trip float parser and adds unit plus HTTP/Arrow regressions.
This resolves that defect; it does not resolve the remaining engine-version
and deployment-catalog differences recorded by #1128.
