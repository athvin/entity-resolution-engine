# Errors

## Contents

- The enforced rule
- Why double-printing happens
- Flattening a chain
- Choosing between anyhow and thiserror
- Self-describing variants
- The exit-code boundary

## The enforced rule

An `#[error("…")]` message must **not** interpolate its `#[from]`/`#[source]`
field, nor a field named `source` (thiserror treats that name as the source
automatically whether or not the attribute is present).

```rust
#[error("reading {path:?}: {source}")]   // wrong — double-prints
#[error("reading {path:?}")]             // right
```

`er-errors/tests/display_no_source.rs` is a posterity gate: it scans every
product source in the workspace and fails the build on a reintroduction, naming
the file, the line and the offending placeholder. It is not a style lint you can
locally `allow`.

The rule stated positively: **name what was attempted; let the chain supply
why it failed.**

## Why double-printing happens

Reporters walk `.source()` themselves. `anyhow`'s `{:#}` does,
`er_server::errfmt::chain` does, and so does anything that prints a cause
chain. If the `Display` of a link also contains its cause, that cause appears
twice:

```text
parsing "x.yaml": bad type at line 3: bad type at line 3
```

With three links it gets quadratic-looking and the actual root cause ends up
buried in repetition — which is the practical cost, not just tidiness.

## Flattening a chain

Because each `Display` is now one link, `e.to_string()` gives you **only the
outermost message**. That is correct when the error is being propagated to a
reporter, and wrong when the error is being turned into a string at rest.

Where a failure becomes a `String` — a job row's failure text, an HTTP error
body, a log line assembled by hand — walk the chain at that point:

| Type | Use |
|---|---|
| `anyhow::Error` | `format!("{e:#}")` |
| a `thiserror` type | `er_server::errfmt::chain(&e)` |

`errfmt::chain` walks `.source()` and joins with `": "`, producing
`outermost: next: innermost`. It is hand-written rather than derived because
the crate does not need thiserror for it, and it has tests for both the
multi-link case and a sourceless error.

**The review heuristic:** every bare `e.to_string()` on a `thiserror` type at a
stringification boundary is a dropped cause. There are instances of this in the
tree — `read::open_org_lake` uses `errfmt::chain` for one failure and
`e.to_string()` for the attach immediately below it — so it is worth checking
rather than assuming.

## Choosing between anyhow and thiserror

| Use | When |
|---|---|
| `thiserror` enum | a library crate whose callers need to **match** on the failure, or anything that maps to an exit code or an HTTP status |
| `anyhow` | a binary's top level and CLI plumbing, where the chain is for a human and nobody matches on it |

`er-errors` is the typed taxonomy that maps to exit codes — see the
er-engine-ops skill. The CLIs use `anyhow` for everything above that.

Do not reach for `Box<dyn Error>` in product code: it loses the ability to
match and gains nothing over `anyhow`.

## Self-describing variants

Where an error message is shown to **whoever wrote the input** — a tenant's
config, a statement YAML — make each variant carry the specifics and the fix,
and avoid a `#[from]` source entirely. `er-compile`'s `CompileError` is the
model: it names the column, the offending token, and the complete list of valid
tokens, because the recipient cannot read the source to work out what was
meant:

```rust
#[error("comparisons.{column} declares level {level:?}, which is not a comparator \
         this engine knows. Known: exact, jaro_winkler:<0..1>, …")]
UnknownLevel { column: String, level: String },
```

That is a different goal from an internal error, where the chain carries the
detail and the top message only needs to say what was being attempted.

## The exit-code boundary

The CLI translates a typed error into an S4.0 exit status. Two things follow:

- A new failure path should be **classified deliberately**. `data` is the
  default for unclassified failures, so falling through to it is a choice that
  makes later triage harder.
- `nothing_to_do` (exit `10`) is the only case with no error class, because it
  is not a failure. If you want a class for it, the operation probably did do
  something.

Retryability (`ErrorClass::retryable()`) is advisory metadata on exactly the two
classes whose cause is outside the run — transient I/O and lock conflict.
Nothing in the CLI acts on it; it exists for an operator or an orchestrator to
read. Do not infer retryability from message text.
