# Evals — observability

Hand-run. Pose each in a **fresh** session; the signal that matters most is
whether this skill triggers at all, and that an adjacent one does not.

## 1. A metric dimension that costs money

**Query:** "Our dashboards can't tell which SQL statement is slow. Add
`statement_name` as a dimension on `er.db.query.duration` so we can group by it."

This is the scenario the whole registry exists for, and the request is
reasonable on its face.

**Must:**

- Refuse the dimension and say why in terms of **billable series** — each
  distinct dimension set is a CloudWatch custom metric, so statement identity
  multiplies one histogram into hundreds of series that grow with every new
  `.sql` file.
- Point out the question is already answerable: `er.statement` is **on the
  span**, where it is free and groupable.
- Offer the `spanmetrics` connector in the collector's values file as the route
  to a per-statement aggregate — a YAML edit, revertible, no Rust change.
- Mention `FORBIDDEN_DIMENSIONS` and that `er-obs/tests/cardinality.rs` would
  fail the change.

**Must not:**

- Add the dimension and regenerate `.claude/skills/observability/references/telemetry-reference.md` to match.
- Treat it as a style preference, or present the cost as hypothetical.
- Hand-edit `.claude/skills/observability/references/telemetry-reference.md` rather than the registry.

## 2. A field that never arrives

**Query:** "I added `span.record("er.tenant_tier", tier)` to the request span
and the field isn't showing up in the span store. The code runs — I put a
`dbg!` next to it."

Two independent causes, and a session that finds one and stops has half an
answer.

**Must:**

- Name the undeclared-field no-op: `tracing` only accepts fields present at the
  `info_span!` callsite, so `record` on a name declared nowhere is **silent**.
  Fix by declaring it with `tracing::field::Empty`.
- Also name the collector's fail-closed `keep_keys` allowlist — a new attribute
  is stripped in flight, so Rust is right and the span store still shows
  nothing.
- Say the name belongs in `er-span/src/attr.rs` with a `Pii` and a
  `Cardinality`, not as a bare string literal at the callsite.

**Must not:**

- Conclude the exporter or the collector is broken and start debugging the
  deployment.
- Stop at the first of the two causes.

## 3. An orphaned trace after a blocking hop

**Query:** "I wrapped the new reconcile entry point in
`tokio::task::spawn_blocking` in the dispatcher. Everything works but the stage
spans show up as separate traces instead of under the job."

**Must:**

- Identify the thread-local current span and `er_obs::spawn_blocking_in_span`
  as the fix.
- Note that nothing else catches this — the spans are all still produced, so it
  reads as tracing not working rather than as a bug — and that
  `er-schema/tests/span_boundaries.rs` is the gate that bans the bare form.
- Finish on `make check`, not `cargo test`.

**Must not:**

- Suggest passing a span or a trace id through the closure by hand.
- Suggest re-parenting with `adopt_traceparent`, which is for a context that
  arrived as a *string* across a process boundary.

## 4. Logging an error, with the PII boundary live

**Query:** "A merge can fail on a constraint violation and we get no detail in
the trace. Add the DuckDB error message to the statement span so we can debug
it from the span store."

The request is the exact thing the `SpansOnly` filter exists to prevent, and the
motivation is legitimate.

**Must:**

- Explain that a DuckDB constraint error carries the offending record value, and
  in a deduplication engine that value *is* the payload.
- Explain that events are deliberately not exported, and that the message
  already reaches stderr, Fluent Bit and CloudWatch — in-account, which is where
  it belongs.
- Point at `er.outcome` / `otel.status_code` as the fact the span does carry,
  and `ER_TRACE_SQL_TEXT` as the deliberate, default-off escape hatch.

**Must not:**

- Add the message as a span field or a span event.
- Claim the information is unavailable anywhere.

## 5. Routing — an adjacent skill should answer

**Query:** "The `standardize` stage takes 30 seconds. Which partition should I
put on the intermediate table?"

**Must:** route to `ducklake-performance` for the measurement discipline and
`ducklake-lakehouse` for the partitioning mechanism.

**Must not:** this skill should not take the question on the strength of the
word "measurement" or because spans are how the 30 seconds was observed.
