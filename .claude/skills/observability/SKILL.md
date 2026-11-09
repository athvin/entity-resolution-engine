---
name: observability
description: Explains how this repo emits telemetry — the `er-span`/`er-obs` split where library crates carry only `tracing` while a binary installs the subscriber, the rule deciding whether a fact belongs on a span, a metric or a log line, the metric registry whose forbidden-dimension list is a cost control rather than a style rule, the PII boundary that makes a bind value or a database error message unexportable, and the gates that enforce all of it. Use when adding a span, metric, counter, gauge, log line or trace attribute, instrumenting a crate or a handler, when `span.record` has no effect, when a span is an orphan root, when a field never reaches the span store, or when a telemetry gate fails. For whether a change is actually faster see ducklake-performance; for reading deployed logs, CloudWatch and the bill see aws-account-operations; for the collector chart itself see infra-platform; for the live health sweep that files board cards see health-sweep.
---

# Observability

The goal here is attribution: one trace from a click in the browser through the
BFF, the control plane, the job row, the dispatcher, the pod, the stage, down to
an individual SQL statement. Most of that spine exists. What breaks it is
always one of the silent failures below, never a compile error.

## The split, and the one rule it creates

- **`er-span`** is the leaf. It carries `tracing` and nothing else, and it only
  **emits**. Library crates depend on this.
- **`er-obs`** **collects**: the subscriber, the OTLP exporter, the metric
  instruments, the layers. **Only a crate that owns an entry point may depend on
  it.**

So when instrumenting the engine, reach for `er-span`. Pulling `er-obs` in
inflates the dependency graph of `er`, `dedupe`, `er-entity` and every test
binary in the workspace — which is the cost the split exists to avoid.

Read that boundary off the manifests rather than from either crate's module doc,
which states it as "only binaries": `er-cli` and `er-server` both have a
`lib.rs` and both take `er-obs`, so the line is entry point versus engine, not
bin versus lib. `references/surface.md` names what sits on each side.

An entry point calls `er_obs::init(Role)` as the **first statement of `main`**
and binds the guard for the life of the process. It writes to **stderr, never
stdout**: `er … --json` is parsed by callers, `er-entity check`'s output is
asserted by tests, and `er-mcp` speaks JSON-RPC on stdout where one stray line
corrupts the protocol.

Any path that can `std::process::exit` must call `guard.force_flush()` first.
Destructors do not run, the batch exporter drops what it holds, and a runner pod
exits immediately after its last span — so the tail of every run, including
every failure, is the part that vanishes.

## Span, metric, or log line

Default to a **span attribute**. It is free, the span store can group by it, and
it needs no registry entry. The table is the decision:

| The fact is | Where it goes | Why |
|---|---|---|
| Identity — statement, relation, org, dataset, run, stage, request, action | a span attribute | free on a span; a metric prices it per series |
| A distribution or rate a dashboard needs | a metric, declared in `er-obs/src/registry.rs` | and only over dimensions drawn from a closed vocabulary |
| Text an operator will read later — an error, a diagnostic | a `tracing::` event | events stay in-account; see failure 3 below |
| Text the person who ran the command reads now | `eprintln!` on stderr | only for an interactive role; see the rule at the bottom |
| A per-stage count | a promoted counter or the free-form payload in `er-obs/src/counters.rs` | it belongs on `run_stages`, not on a metric |

"Lean towards over-logging" cashes in at the **filter**, not at the call site:
our own crates sit at `debug` in `er_obs::init::DEFAULT_FILTER`, and the lever
against volume is that directive rather than the absence of the call. The same
directive pins the noisy third-party targets, and that pinning is a cost control
— `hyper`, `h2`, `rustls`, `reqwest`, `tokio_postgres`, `aws_smithy_runtime` and
`kube` have all been emitting into a void, and CloudWatch bills by the gigabyte.
Dropping one from the directive holds it at `info` on the next deploy.

## The silent failures

Every one of these produces correct-looking code, a green build and a green
deploy. None of them is a compile error, and most are invisible until someone
goes looking for a field that is not there.

**1. `span.record` on a field not declared at the callsite does nothing.**
`tracing` only accepts fields present at the `info_span!` site, so recording an
undeclared name is a **silent no-op**. This shipped: neither `otel.status_code`
nor `error.type` was declared, so two of three lines in `er_span::check`'s error
arm had no effect, and because `otel.status_code` is what the collector's
`errors` tail-sampling policy reads, every failed statement fell to the residual
and was mostly discarded. Declare every field you will later record, with
`tracing::field::Empty`.

**2. Bare `tokio::task::spawn_blocking` orphans every span the closure makes.**
`tracing`'s current span is thread-local. Use
`er_obs::spawn_blocking_in_span`, which is otherwise identical — same
signature, same `JoinHandle`, same panic-as-`JoinError`. This matters more here
than elsewhere because DuckDB is synchronous, so the whole pipeline runs inside
closures the async dispatcher hands off; miss one hop and every stage and
statement span below it detaches from the job. Nothing else catches it — the
spans are all still produced, so it does not read as a bug, it reads as tracing
not working.

**3. Events are not exported, deliberately.** The `SpansOnly` per-layer filter
sets `event_enabled → false` on the OTel layer only, because
`tracing-opentelemetry` turns any event inside a span into a span event with its
fields — and a database error message routinely contains the offending record
value, which in a deduplication engine *is* the payload. So a
`tracing::error!(error = %e)` reaches stderr, Fluent Bit and CloudWatch, and
never the span store. That is the design: do **not** "fix" a missing error
message in the span store by promoting it to a span field. The span carries the
*fact* of the failure — `er.outcome` and `otel.status_code` — which is what
dashboards and the sampling policy actually read.

**4. The collector's attribute allowlist is fail-closed.** `keep_keys` in
`infra/k8s/observability/otel-collector-values.yaml` drops every span attribute
not named in it. Add an attribute in Rust and forget the YAML and it is stripped
in flight: the code is right, the deploy is green, the span store simply never
shows the field. Edit both in the same change.

**5. A disabled span does not suppress an event.** Returning `Span::none()` to
skip a probe stops the span and not the log line — and the *line* is what
CloudWatch bills for. Any response hook needs
`if span.is_disabled() { return; }` at the top.

**6. The sampler default is not always-on.** `SdkTracerProvider` defaults to
`ParentBased(AlwaysOn)`, which honours a remote parent's sampled flag — so once
a span adopts an inbound `traceparent`, any caller could silence this process's
telemetry with `…-00`, invisibly. `otlp.rs` states `Sampler::AlwaysOn`; leave it
stated. Sampling belongs to the collector's `tail_sampling`, which can see a
trace's outcome before judging it.

**7. A process can look installed and export nothing.** Observed live (#941):
an er-api pod logged `OTLP span export installed`, recorded request spans,
exported **zero spans for 40 minutes** — and its metric pipeline flowed to the
same collector the whole time. Nothing paged, because nothing watched span
volume. The `er.telemetry.spans_ended` / `spans_exported` /
`span_exports_failed` counters now make the process testify against itself:
ended rising while exported is flat puts the wedge in-process, failures rising
puts it on the collector or the network, and
`infra/terraform/envs/dev/alarms-telemetry.tf` fires on the divergence. When a
span goes missing, read those counters before re-deriving any of the six
failure modes above.

## Adding a metric

A metric is the expensive option, so it is also the one with a registry. Each
new metric is **an entry in `er-obs/src/registry.rs` plus a call site** — not new
machinery.

```
# after editing the registry, regenerate the reference and COMMIT it
cd rust && UPDATE_EXPECT=1 cargo test -p er-obs reference
```

- Every dimension must come from a **closed vocabulary that exists in code**,
  and `Cardinality::Bounded` has to name where that vocabulary lives — a bound
  nobody can check is a bound nobody maintains.
- State `series` as the product of those cardinalities. It is written down rather
  than computed so a reviewer can disagree with what the author believed.
- `FORBIDDEN_DIMENSIONS` is a cost control. Under `awsemf` each distinct
  dimension set is a billable CloudWatch custom metric, so statement identity,
  tenancy, run identity and record identity would turn one histogram into
  hundreds of series that grow with every new `.sql` file. All of them are
  available on **spans**, where they cost nothing.
- Want a per-statement, per-tenant or per-action aggregate anyway? That is a
  `spanmetrics` connector in the collector's values file — a YAML edit,
  revertible, with no Rust change.

[`references/telemetry-reference.md`](references/telemetry-reference.md) is
**generated** from the registry by `cargo test -p er-obs reference` and
byte-compared, so read the declared metrics and the forbidden list there rather
than from any prose. A metrics table written by hand is how a metric ends up
documented and never emitted, which reads as coverage — its dashboard panel
renders an honest-looking empty chart.

Whatever the registry knows, the **emission site must match it**: the arithmetic
is only true if the code does not add a `KeyValue` the registry never declared.
Assert the declared dimension list against the call site, the way
`metrics.rs` already does.

## Adding a span attribute

Declare the name in `er-span/src/attr.rs` with a `Pii` and a `Cardinality` — the
vocabulary is values, not scattered string literals, so a name cannot drift
between a dashboard, a runbook and the code. Then add it to the collector
allowlist, per failure 4.

`Pii::Never` marks a name that **must not exist**, declared so the boundary is
visible and a gate can assert nothing emits it.

## The PII rule is structural, not advisory

This is a deduplication engine: names, emails, phones and addresses *are* the
payload. Telemetry carries identifiers and counts, and never attribute values.
The enforcement is shape rather than a per-call-site audit, because an audit goes
stale:

- `er_span::sql_span` **takes no parameter argument**. There is nowhere to put a
  bind value, so a caller cannot record one by mistake.
- Only **generated** code may call `sql_span`. Product code calls
  `generator_span`, which carries the generator's identity and a bounded summary
  and has no `sql` parameter at all — because runtime-assembled SQL can embed
  config-derived literals, which are tenant metadata.
- `ER_TRACE_SQL_TEXT` gates the statement template **and** the error message,
  for the same reason: both can carry payload. Off by default.
- A request span records `url.path` and never `url.query`, because the
  golden-records search takes a contains-query over record values.

When adding a field, ask what it holds on a bad day, not a good one. A clean
verification run cannot find a leak that only a constraint violation produces.

## The frontend half

The click is the root of the trace, and the vocabulary of clicks is **closed**.

- `src/lib/telemetry/actions.ts` declares every reportable interaction. Mark a
  control with `data-er-action="…"`, or call `recordAction("…")` for the
  keyboard path — the review inbox and labelling surfaces are keyboard-first, so
  a click-only listener records nothing for the steward who resolves the most
  pairs.
- The argument must be a **plain string literal**. Never source an action name
  from `data-testid` or `aria-label`: those interpolate emails and record keys
  by design (`aria-label={`Remove ${member.email}`}`), they are correct as
  accessibility, and accessibility wins.
- `scripts/check-er-actions.mjs` fails the frontend gate in **both** directions —
  an undeclared name that would silently never record, and a declared name
  nothing emits, which reads as coverage.
- The proxy validates the inbound `traceparent` against the same rule
  `er_server::acceptable_traceparent` applies and **mints a fresh one** when the
  browser's would not survive, because a new trace beats a broken one. A
  rejected header fails silently on the Rust side, which is why the TypeScript
  mirrors the rule rather than forwarding whatever arrived.
- Per-request context rides `AsyncLocalStorage`, not a parameter, so no route
  handler has to thread it and a handler that bypasses `respond()` degrades to a
  *fresh* trace rather than to none.

## The gate

```
make check       # backend finish line; the obs gates run in its test phase
make frontend    # the action vocabulary gate and the rest of the frontend tier
```

The telemetry gates are **ordinary Rust tests**, so `cargo test` does run them —
which is worth stating plainly, because the house rule that the cargo trio is
insufficient is about `entities-check` and `skills-check`, not about these. What
the trio actually misses here is the **generated reference**: `skills-check`
catches a stale `references/surface.md`, and `er-entity verify` is unrelated to
any of it. See `rust-conventions` for why `make check` is still the finish line.

To run one gate rather than the suite, note each is a module of its crate's
single test binary — `cargo test -p er-obs --test main cardinality`.

[`references/surface.md`](references/surface.md) — generated: the roles and the
`service.name` each reports as, the gates that enforce the rules above, and the
derived gap between what emits structured events and what still only reaches
stderr.

[`references/verifying.md`](references/verifying.md) — how to prove a producer
end to end from a laptop with no deploy, the traps that make a verification run
lie, and why a clean run cannot find the defects that matter.

## Old patterns, and what is deliberately unwired

- **`eprintln!` is not uniformly the pre-`tracing` idiom**, and "convert them
  all" is the wrong instinct. Ask `er_obs::Role::is_unattended`: for an
  **unattended** role (`Api`, `Dispatcher`, `Task`, `Mcp`) nobody reads that
  stderr as the output of a command they typed, so a diagnostic must be a
  structured event or it is consumed later with no ancestry and no target to
  filter by. For an **attended** role (`Cli`, `EntityCli`, `Dedupe`) stderr *is*
  the product — `er-entity check`'s output is asserted by tests — and routing it
  through the subscriber would wrap it in JSON and put it behind `RUST_LOG`,
  where a narrow filter makes it vanish. Both idioms in one file is the expected
  end state, not a half-finished migration.

  The predicate is **not** "does it run in the cluster". `Mcp` is unattended
  without being deployed at all — `er-mcp` is in no Dockerfile and no chart, and
  an MCP client spawns it over stdio — which is the arm that forced the rename
  from `is_deployed`. `references/surface.md` derives both the split and which
  roles ship, so neither can drift from the source.

  That `er-mcp` ships nowhere is a **decision, not a gap**: stdio has nothing
  attached to its stdin in a pod, and its API key is one org's, so a shared
  deployment would be a tenant-isolation bug. Do not "fix" its telemetry by
  giving it a workload — point `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT` at a
  collector locally instead. The reasoning is on `Role::Mcp` itself.
- **`er_obs::logging::stage_record_line` has no callers on purpose.** Its key
  set is validated exactly against the promoted counters, and calling it with
  those defaulted to zero would fill `run_stages` with zeros that every
  dashboard would read as measurements. It lands with the `run_stages` writer.
- **Schema-reconciliation spans are orphan roots** — they are emitted before a
  run span exists, so that work is not attributable to the run that triggered
  it. Correct today, and worth knowing before concluding the spine is broken.
