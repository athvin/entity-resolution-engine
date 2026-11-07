# The instrumentation surface

Derived from the workspace. Prose outside the GENERATED markers survives
regeneration; do not hand-edit inside them.

For the declared **metrics** and the dimensions a metric may never carry, read
`docs/telemetry-reference.md` instead — it is generated from
`er-obs/src/registry.rs` by that crate's own test and byte-compared, so it
cannot drift from what is emitted. It is reached from the skill body.

## Contents

- [Roles](#roles) — what each entry point reports as
- [The gates](#the-gates) — what enforces the rules, and where
- [Structured events versus bare stderr](#structured-events-versus-bare-stderr) —
  the derived adoption gap

<!-- BEGIN GENERATED: obs-surface -->
### Roles

What each entry point passes to `er_obs::init`, and the
`service.name` and `er.role` it therefore reports as:

| `Role` | `service.name` |
|---|---|
| `Api` | `er-api` |
| `Dispatcher` | `er-dispatcher` |
| `Task` | `er-task` |
| `Cli` | `er-cli` |
| `EntityCli` | `er-entity` |
| `Dedupe` | `dedupe` |
| `Mcp` | `er-mcp` |

### The gates

Every test enforcing a rule in the skill body, with the first line of
its own module doc. All of them run under `make check` and none under
the bare cargo trio:

- `er-obs/tests/adopts_parent_context.rs` — A job span inherits the trace id of the request that enqueued it.
- `er-obs/tests/cardinality.rs` — No metric may carry a dimension that costs money without anyone choosing to.
- `er-obs/tests/no_pii_in_exported_spans.rs` — A record value must not reach an exported span, asserted on the bytes.
- `er-obs/tests/otlp_export.rs` — A span, over a real socket, in both of this workspace's runtime shapes.
- `er-obs/tests/reference.rs` — `docs/telemetry-reference.md` is generated from the registry, byte for byte.
- `er-schema/tests/collector_allowlist.rs` — The collector's attribute allowlist still covers what the code emits.
- `er-schema/tests/span_boundaries.rs` — The posterity gate for blocking hops: no bare `spawn_blocking` in the control plane.
- `er-schema/tests/sql_span_pii.rs` — The PII boundary on spans, enforced rather than documented.

### Structured events versus bare stderr

A `tracing` event carries its span ancestry, so it is attributable to a
run, a job and a tenant; an `eprintln!` is a line on stderr and nothing
more. Fluent Bit ships both, so neither is lost — but only the first can
be grouped, filtered by target, or joined to a trace.

**Product files emitting structured events:**

- `er-cli/src/lib.rs`
- `er-obs/src/identity.rs`
- `er-obs/src/init.rs`
- `er-obs/src/otlp.rs`
- `er-obs/src/propagate.rs`
- `er-server/src/dags/control/mod.rs`
- `er-server/src/dags/tasks.rs`
- `er-server/src/dispatcher.rs`
- `er-server/src/dispatcher_bin.rs`
- `er-server/src/exec_task.rs`
- `er-server/src/executor.rs`
- `er-server/src/fixups.rs`
- `er-server/src/lib.rs`
- `er-server/src/mail.rs`
- `er-server/src/main.rs`
- `er-server/src/pg.rs`
- `er-server/src/reporter.rs`
- `er-server/src/satoken.rs`
- `er-server/src/shutdown.rs`
- `er-server/src/telemetry.rs`
- `er-span/src/lib.rs`

**Product files still writing `eprintln!` and no structured event.**
This is the gap worth reading, and it is not all defect: a CLI's
user-facing error output is correctly an `eprintln!`, while a diagnostic
a dashboard should see is not. Judge each by whether anything downstream
needs to group it:

- `dedupe-cli/src/main.rs`
- `er-cli/src/cli.rs`
- `er-cli/src/profile.rs`
- `er-entity-cli/src/main.rs`
- `er-mcp/src/main.rs`
- `er-server/src/bin/er-openapi-check.rs`

**Mid-conversion — both idioms in one file:**

- `er-cli/src/lib.rs`
- `er-obs/src/init.rs`
- `er-server/src/mail.rs`
- `er-server/src/main.rs`
<!-- END GENERATED: obs-surface -->
