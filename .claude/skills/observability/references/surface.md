# The instrumentation surface

Derived from the workspace. Prose outside the GENERATED markers survives
regeneration; do not hand-edit inside them.

For the declared **metrics** and the dimensions a metric may never carry, read
`docs/telemetry-reference.md` instead — it is generated from
`er-obs/src/registry.rs` by that crate's own test and byte-compared, so it
cannot drift from what is emitted. It is reached from the skill body.

## Contents

- [Which side of the split each crate is on](#which-side-of-the-split-each-crate-is-on) —
  who emits, who collects
- [Roles](#roles) — what each entry point reports as
- [The gates](#the-gates) — what enforces the rules, and where
- [Structured events versus bare stderr](#structured-events-versus-bare-stderr) —
  the derived adoption gap

<!-- BEGIN GENERATED: obs-surface -->
### Which side of the split each crate is on

**Depend on `er-span` — they emit and collect nothing.** Instrumenting
one of these must not reach for `er-obs`:

- `dedupe-core`
- `er-entities`

**Depend on `er-obs` — they own an entry point and install the
subscriber.** Note this is not the same as "binary crates": some of
these have a `lib.rs` too, which is why both crates' module docs
describing the line as bin-versus-lib are imprecise:

- `dedupe-cli`
- `er-cli`
- `er-entity-cli`
- `er-mcp`
- `er-server`

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
its own module doc. Each is a **module of its crate's single test
binary**, not a target of its own, so run one with
`cargo test -p <crate> --test main <module>`:

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
- `er-mcp/src/main.rs`
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

The set below is split by `er_obs::init::Role::is_deployed`, because
whether a bare `eprintln!` is a defect depends entirely on whether
anyone is watching the terminal. Deployed roles are
`Api Dispatcher Mcp Task`;
interactive ones are
`Cli Dedupe EntityCli`.

#### The actual gap — a deployed process writing unstructured stderr

Nobody reads these terminals. Each line here reaches an operator only
through Fluent Bit, with no span ancestry to attribute it and no target
to filter it by. **These are worth converting:**

**None.** Every deployed role emits structured events.

#### Correct as-is — interactive output

Here stderr **is the product**: `er-entity check`'s output is asserted
by tests, `er-entity lint` prints diagnostics a human acts on, and
`dedupe`'s "wrote N rows" is read by whoever typed the command.
Routing these through the subscriber would wrap them in JSON and put
them behind `RUST_LOG`. **Do not convert them:**

- `dedupe-cli/src/main.rs` — `Role::Dedupe`
- `er-cli/src/cli.rs` — `Role::Cli`
- `er-cli/src/profile.rs` — `Role::Cli`
- `er-entity-cli/src/main.rs` — `Role::EntityCli`

#### Cannot emit — no subscriber installed

These binaries never call `er_obs::init`, so a `tracing` event
would go nowhere. `eprintln!` is the only channel they have:

- `er-server/src/bin/er-openapi-check.rs` — installs no subscriber

**Mid-conversion — both idioms in one file.** Expected and not a
defect: a deployed role's operational diagnostics become events while
its genuine user-facing output stays an `eprintln!`.

- `er-cli/src/lib.rs`
- `er-obs/src/init.rs`
- `er-server/src/mail.rs`
- `er-server/src/main.rs`
<!-- END GENERATED: obs-surface -->
