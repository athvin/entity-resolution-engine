# Instrumentation: does this path owe telemetry?

`observability` is the authority on **how** to emit. This file is only about
**where it is owed**, which is the half no gate can decide — the gates here all
check that what *is* emitted is well-formed, and none of them notices an absence.

## Contents

- [The debt test](#the-debt-test)
- [The four silent no-ops worth finding in a diff](#the-four-silent-no-ops-worth-finding-in-a-diff)
- [Span, metric or event — the finding has to pick](#span-metric-or-event--the-finding-has-to-pick)
- [PII is the one place to be noisy](#pii-is-the-one-place-to-be-noisy)
- [Already gated — do not raise](#already-gated--do-not-raise)

## The debt test

Instrumentation is owed where **a question someone will ask at 2am cannot be
answered from what the code emits.** Three shapes qualify, and nothing else
does:

1. **A new unit of work** — a handler, a task body, a stage, a dispatcher loop
   iteration. It owes a span, because a span is what joins it to the request or
   the job above it. The span is free; this is the cheapest debt in the list.
2. **A new way to fail** — a new error variant, a new early return, a new
   refusal. It owes an outcome on the span (`er.outcome`,
   `otel.status_code`) and, where there is text a person needs, a
   `tracing::` event. The collector's tail-sampling `errors` policy reads
   `otel.status_code`; a failure that does not set it falls to the residual and
   is mostly discarded, so the failure path is the one most likely to be
   missing *and* the one most expensive to lose.
3. **A new rate or distribution a dashboard will want** — and only then a
   metric, which is the expensive option and the one with a registry.

**State the consumer or drop the finding.** Name the dashboard panel, the
sampling policy, or the join that goes blind. A span attribute nothing reads is
not free: it costs a collector-allowlist edit, and when that edit is forgotten
the field is stripped in flight and the code merely *looks* instrumented.

## The four silent no-ops worth finding in a diff

Each produces correct-looking code, a green `make check` and a green deploy.
Confirmed un-gated — grepping the test tree for a scanner over any of these
finds nothing.

- **`span.record` on a field the callsite never declared does nothing.**
  `tracing` only accepts fields present at the `info_span!` site, so recording
  an undeclared name is dropped silently. This shipped once already: neither
  `otel.status_code` nor `error.type` was declared, so two of three lines in an
  error arm had no effect. In a diff, read the `record` call against its span
  macro and check the name is there with `tracing::field::Empty`.
- **A `process::exit` path with no `guard.force_flush()` before it** loses the
  tail of the run. Destructors do not run, the batch exporter drops what it
  holds, and a runner pod exits right after its last span — so the part that
  vanishes is every failure. The flush *mechanism* is tested; the *call sites*
  are not enumerated anywhere, which is why a new exit path is a review matter.
- **A disabled span does not suppress an event.** Returning `Span::none()` to
  skip a probe stops the span and not the log line, and the line is what
  CloudWatch bills for. A new response hook needs
  `if span.is_disabled() { return; }` at the top.
- **A new `KeyValue` at a metric call site** that the registry never declared
  breaks the series arithmetic the registry states. `metrics.rs` asserts the
  declared list against the code, so this is gated *there* — but a **new
  metric** whose declared `series` product is simply wrong is not, because the
  number is written down rather than computed, on purpose, so that a reviewer
  can disagree with it. Recompute it.

## Span, metric or event — the finding has to pick

A finding that says "add observability here" is not actionable. The decision
table lives in `observability`; the compressed form is that identity goes on a
**span** (free, groupable), a distribution a dashboard needs goes in the
**registry** (priced per series), text a person reads goes in a `tracing::`
**event** (stays in-account, never exported), and a per-stage count belongs on
`run_stages` rather than on a metric.

The expensive mistake in that table is reaching for a metric. Under `awsemf`
every distinct dimension set is a billable custom metric, which is why
statement, org, dataset, run and record identity are forbidden dimensions — all
of them are available on spans for nothing. So **"this should be a metric"
needs the series arithmetic in the finding**, and "this should be a span
attribute" almost never needs defending.

## PII is the one place to be noisy

This is a deduplication engine: names, emails, phones and addresses *are* the
payload. Telemetry carries identifiers and counts, never attribute values.

A new path that puts a record value, a bind parameter, a database error message
or a query string where telemetry can reach it is **always reportable**, even
though the two known export paths are gated on the bytes. The reason to report
it anyway, breaking this skill's own do-not-duplicate-a-gate rule: the gate
asserts on what leaves the process, and a value written to a log line, a metric
label, a spill directory, a test fixture or an error string that a human later
pastes into a ticket never leaves through that door. Ask what the field holds on
a bad day — a constraint violation quotes the offending value — rather than what
it holds in a clean run.

The frontend half of the same rule: an action name must be a plain string
literal from the declared vocabulary, never sourced from `data-testid` or
`aria-label`, which interpolate emails and record keys by design and are correct
to do so.

## Already gated — do not raise

- A bare `tokio::task::spawn_blocking` in the control plane, which orphans every
  span the closure makes — `er-schema/tests/span_boundaries.rs`.
- An `eprintln!` in `er-server` — `er-schema/tests/structured_logging.rs`. Note
  the deliberate carve-outs: `er-entity check`'s diagnostics, `er … --json`,
  the stage line, and `er-mcp`'s JSON-RPC stdout are a human or machine
  contract, not telemetry, and converting one is a regression.
- A span attribute missing from the collector's `keep_keys` allowlist —
  `er-schema/tests/collector_allowlist.rs`.
- A forbidden metric dimension, and a call site that has drifted from the
  registry — `er-obs/tests/cardinality.rs` and `er-obs/src/metrics.rs`.
- A record value in an exported span — `er-obs/tests/no_pii_in_exported_spans.rs`,
  asserted on the raw protobuf.
- An undeclared or never-emitted frontend action name — `check-er-actions.mjs`
  fails `make frontend` in both directions.
- `eprintln!` outside `er-server`: in scope for conversion but **not a review
  finding**. `observability`'s generated `references/surface.md` derives where
  it still is, which is the list to work from.
