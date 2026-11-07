# Reuse: was a copy written of something that carries an invariant?

"This is duplicated" is not a finding worth a review cycle, and the bundled
`/simplify` already hunts duplication and **applies** the cleanup. The finding
this dimension exists for is narrower and much more expensive:

> **A hand-rolled copy of a house helper, where the helper carries an invariant
> the copy silently drops.**

The copy compiles, reads better than the helper in isolation, passes every test,
and is wrong in a way that only shows up in production or at 2am. That is the
whole category.

## Contents

- [The helpers that carry invariants](#the-helpers-that-carry-invariants)
- [The harnesses that carry invariants](#the-harnesses-that-carry-invariants)
- [Reuse the gate shape, not just the code](#reuse-the-gate-shape-not-just-the-code)
- [What is not a finding](#what-is-not-a-finding)

## The helpers that carry invariants

| A diff that writes | Should reach for | The invariant the copy drops |
|---|---|---|
| an inline Postgres statement against a declared relation | the generated `er_entities::<relation>::{read,create,update}::*` function | the type mapping, the `RETURNING` list, the analyser's plan check, the oracle's type verdict |
| `e.to_string()` on an error being flattened into a `String` or an HTTP body | `er_server::errfmt::chain(&e)`, or `format!("{e:#}")` for `anyhow` | the rest of the chain. Display must not interpolate its own source, so the outermost message alone is all `to_string()` yields — the cause is simply gone |
| `Catalog::load_dir` in something that writes a generated artifact | `er_entity_lint::load_linted` | every V-rule; `load_dir` checks structural coherence only |
| a lake read assembled as a string | a declared statement, or the er-golden merge-plan generator | see `declared-sql.md` — and this one is gated, so it is a red test rather than a finding |
| a `sql_span` call from product code | `er_span::generator_span` | `sql_span` exists for generated code; runtime-assembled SQL can embed config-derived literals, which are tenant metadata |
| a shell snippet inside a workflow `run:` block | `scripts/ci/<name>.sh`, called as `run: bash scripts/ci/<name>.sh` | testability. Gated as a budget: more than three lines, or **any** control flow at any length |
| a chart, with a library | hand-rolled DOM/CSS | the monochrome rule and the absence of a charting dependency are both decisions — see `frontend-data-ui` |
| a UI primitive, by hand | the shadcn CLI | the token system; a hand-rolled primitive is where a palette utility gets reintroduced, which ESLint then rejects |
| a browser call to the control plane | the server-only upstream client through the BFF | the browser never reaches `er-server`; see `frontend-bff` |

The column that matters is the third one. A finding that cites only the first
two — "there is a helper for this" — is a style note. Name the invariant and the
finding becomes checkable.

## The harnesses that carry invariants

Test-side reuse is where the silent-direction failures live, because a test that
does not notice is indistinguishable from a clean tree.

- **A new `pg_*.rs` uses `common`'s one-database-per-test harness.** The
  previous idiom — connect to the shared database, `DROP TABLE … CASCADE` a
  hand-maintained list, rebuild — had drifted into ten distinct drop-list
  variants across thirty-five files, and a list like that is wrong in the
  direction nobody sees: a table missing from it survives into the next test.
  A new file that reintroduces a drop list is the finding.

  The adjacent rule is **gated, so do not raise it**: every `pg_*.rs` must be
  one `#[tokio::test]` with sequential phases, and `pg_serial.rs` fails on a
  second one. Read why before concluding the harness made it obsolete — the
  per-database harness changed the *reason* and kept the rule, because
  `fresh_db` is keyed per file, so two tests in a file ask for the same database
  and the second `DROP DATABASE … WITH (FORCE)` disconnects the first mid-test.
  The race moved from the tables to the database; it did not go away.
- **A new security regression test goes in the harness `security`'s harness-map
  names**, not in a new file beside it. A security test in a fresh file with no
  tier variable is a test that never runs in CI.
- **A test asserting an exported-telemetry property asserts on the bytes**, the
  way `no_pii_in_exported_spans.rs` does. A field that is set but filtered
  before export is harmless; a field absent from the span but present in the
  payload is not, and only the payload can tell those apart.
- **A test for a frontend change belongs in the tier `frontend-testing`
  assigns it.** A new Playwright spec for something a component test could
  assert buys the slowest tier's cost for the fastest tier's coverage.

## Reuse the gate shape, not just the code

This repo has a established shape for "this class of bug cannot come back": a
source scan in a test file, with its own self-check, that fails loudly on an
empty walk. `egress_guard.rs` is the named exemplar and
`control_tasks_no_engine.rs`, `no_second_executor.rs`, `span_boundaries.rs` and
`freeform_sql.rs` are all built to it.

So when a diff **fixes a class** rather than an instance — the second occurrence
of a bug, or a rule stated in a comment or a module doc — the reviewable
question is whether it reused that shape. A fix with no structural claim behind
it is an invitation to the third occurrence, and the precedent is cheap: the
scanner is twenty lines and needs no toolchain.

Two cautions, so this does not become a demand attached to every diff:

- A gate is trusted only once it has been **observed failing**. A diff that adds
  a scanner and never broke it on purpose has added something that reads like a
  gate — and a detector breaks by noticing nothing, which looks exactly like a
  clean tree. Ask for the observation, not for more assertions.
- One instance is an instance. Do not ask for a gate on first occurrence.

## What is not a finding

- Duplication with no invariant behind it. Route it to `/simplify`, which fixes
  rather than reports.
- A second implementation that is deliberately independent. The frontend's
  `traceparent` validation mirrors `er_server::acceptable_traceparent` **on
  purpose**, because a rejected header fails silently on the Rust side.
  Likewise the licence allowlist is mirrored between Rust and npm by design.
- Three lines of similar-looking code. The repo's own threshold for extracting
  shell is three lines *plus* any control flow; that is a reasonable prior for
  Rust too.
- A bare `tokio::task::spawn_blocking` where `er_obs::spawn_blocking_in_span`
  was wanted. Real, and gated — `er-schema/tests/span_boundaries.rs` fails on it.
- `Cargo.toml` edited by hand rather than through `cargo add`. The result is
  byte-identical, so there is nothing for a reviewer to see.
