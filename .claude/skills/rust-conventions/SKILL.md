---
name: rust-conventions
description: The authoritative Rust conventions for this workspace — that `make check` (not the bare cargo trio) is the finish line, the rule that an error's Display must never interpolate its own `source` and how to flatten a chain instead, the clippy allow policy, the MSRV and edition-2024 consequences, and the TLS/AWS feature pins that must not be relaxed. Use before authoring or reviewing any Rust in this repo, when deciding what to run before calling backend work done, when adding a dependency or a feature, when writing an error type, or when a clippy or doc warning fails the build. Supersedes any generic Rust skill's finish-line checklist. For schema or SQL changes see entity-framework; for the control plane see er-server-control-plane; for why that CI job failed, the workflow topology, and the advisory, licence and ban gates a dependency bump must clear, see ci-and-supply-chain; for the AWS SDK surface see aws-account-operations.
---

# Rust in this workspace

The cargo workspace root is **`rust/`**, not the repo root. Bare `cargo`
commands must run from there, which is why every Makefile target does `cd rust`
first.

## The finish line is `make check`

```
make check
```

Run it from the **repo root** before calling backend work done. It is the
PR-equivalent gate and mirrors the `rust` workflow's lint, entities and test
jobs. The step list is derived from the `check:` target, in
[references/gates.md](references/gates.md) — do not restate it from memory,
because it has grown twice.

**`cargo fmt` + `cargo clippy` + `cargo test` on their own are not
sufficient.** They skip `entities-check`, the entity-framework gate that
regenerates every artifact from `entities/`, fails on a stale committed file and
runs the E0xx statement passes; and they skip `skills-check`, which is the same
class of gate over `.claude/skills/**`. A change can pass clippy and every test
and still break either one.

> **Precedence.** Some installed third-party skills — notably
> `rust-best-practice`, which symposium syncs into `rust/.claude/skills/` —
> prescribe a generic finish line of `cargo fmt` + `cargo clippy --all
> --workspace` + `cargo test --all --workspace`, and that skill asks to be
> activated before any Rust work. In this repo that checklist is **incomplete**.
> This skill supersedes it on the finish line. Its other guidance (and the
> error-handling skills it ships alongside) is fine and does not conflict —
> `library-errors` in particular agrees with the Display rule below.

Its `doc` step is `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
--document-private-items`, so a doc comment linking to a **private** item is a
hard failure too — that flag is why, and it is the case that used to pass
locally and fail CI.

The gap that remains is the gated tiers: Postgres- and DuckLake-backed tests
**self-skip** without their env vars, so a green `make check` says nothing about
them. See [references/gates.md](references/gates.md).

## Errors: never interpolate your own source

This is enforced, not advisory. `er-errors/tests/display_no_source.rs` scans
every product source in the workspace and fails the build on a reintroduction,
naming the file, line and placeholder.

```rust
#[error("reading {path:?}: {source}")]   // wrong — double-prints
#[error("reading {path:?}")]             // right
```

The rule: **name what was attempted, and let the chain supply why.** Reporters
walk `.source()` separately, so interpolating the cause prints it twice:

```text
parsing "x.yaml": bad type at line 3: bad type at line 3
```

It applies to a field named `source` too — thiserror treats that name as the
source automatically, with or without the attribute.

**The flip side, which is the part people miss.** Because `Display` is now one
link, `e.to_string()` yields only the outermost message. Where an error is
flattened into a `String` rather than propagated to a reporter, walk the chain
at that point:

| Type | Flatten with |
|---|---|
| `anyhow::Error` | `format!("{e:#}")` — the alternate formatter walks the chain |
| a `thiserror` type | `er_server::errfmt::chain(&e)` |

`er-server` does this for job-failure text and HTTP bodies, because those are
strings at rest, not errors being reported. A bare `e.to_string()` there
silently discards the cause. Error modelling in full:
[references/errors.md](references/errors.md).

## Lint policy

`[workspace.lints]`, applied per-crate via `[lints] workspace = true`:

- `unsafe_code = "deny"` — workspace-wide, no exceptions.
- clippy `all` and `pedantic` both at `warn`, with an explicit allow-list.

CI runs `clippy --workspace --all-targets -- -D warnings`, so **every warning
is a hard failure**, including in tests and benches.

Each allow carries its reason as a comment beside it, and two of them protect
**correctness** rather than taste — both read like bugs to anyone meeting them
cold, so do not "fix" either:

- **`float_cmp`.** Trained and fixed float parameters are compared for exact
  equality, deliberately. An epsilon tolerance here makes a parameter mismatch
  invisible, which is the failure it would be introduced to hide.
- **`unreadable_literal`.** The determinism-critical float and hash constants
  are transcribed values; reformatting them risks a transcription error for zero
  functional gain.

The full allow-list with each comment is derived from
`[workspace.lints.clippy]`: [references/crate-map.md](references/crate-map.md).

Do not add a new `#[allow]` at a call site to silence a pedantic lint without
saying why in a comment — the workspace block sets the precedent that an allow
carries its reason, and the generated table shows a blank cell for any that
does not.

## MSRV, edition, and what follows from them

- The workspace pins both; the values are in
  [references/crate-map.md](references/crate-map.md) rather than here, because
  an MSRV in prose outlives the bump that moved it.
- Nothing verifies the floor downward: `rust-toolchain.toml` asks for `stable`
  and CI installs stable. Its live effect is **dependency resolution**, so
  raising it is a dependency decision, not a syntax one.
- Edition 2024 makes `std::env::set_var` **`unsafe`**, and `unsafe_code` is
  denied — so a test cannot write the process environment. Code that reads env
  needs an injectable seam (a lookup closure, a thread-local overlay) to be
  testable. `er-env` has the overlay; newer code takes a lookup function.

## TLS and AWS feature pins

These are not style choices; relaxing one breaks the build image or the pod.

- **`rustls` with `ring`**, pinned explicitly (`default-features = false`,
  features `ring`, `std`, `tls12`, `logging`). The provider is named so the
  Postgres TLS verifier and the `ClientConfig` agree on one — a mismatch is a
  runtime panic, not a compile error.
- **`reqwest` is `rustls-tls` only**, `default-features = false`. No OpenSSL
  anywhere.
- **`aws-sdk-*` must stay `default-features = false`** and live only in
  `er-server`'s manifest, never the workspace table. Defaults resolve TLS to
  aws-lc-rs, which needs cmake the build image lacks; and `sso` /
  `credentials-process` pull credential providers the pod cannot use, because
  it authenticates by EKS Pod Identity (the container-credentials provider).
- Consequence for local work: **`aws sso login` credentials will not work for a
  local `er-server`.** Use `ER_MAIL_BACKEND=log`.
- Postgres TLS goes through `er-pg-tls`, which gives `sslmode=require`
  semantics — encrypt, do not verify — because the RDS CA is not in system
  roots.

## Dependencies

**Use `cargo add`**, never hand-edit `Cargo.toml`. Run it from `rust/`.

Two pins are locked to another crate's choice and cannot be bumped
independently:

- `arrow` must equal the major the `duckdb` crate pulls.
- `datafusion` must equal the major `ff-analysis` resolves.

A PR that bumps either alone can never go green, and the unblocker is upstream —
a `duckdb` release for `arrow`, a new `ff-analysis` tag for `datafusion`.
`.github/dependabot.yml` excludes both from major updates for exactly this
reason, each with the compiler error that proved it in a comment, so the bot no
longer proposes them. (`@playwright/test` is excluded too, for the image-tag
reason the frontend-testing skill explains.)
`duckdb` itself is an **`=` pin** so that a DuckDB change is always a
deliberate, reviewable event — it can move emitted SQL and floating-point
output, which the goldens compare.

The current crate inventory and version pins are generated, not written here:
[references/crate-map.md](references/crate-map.md).

A new dependency also has to clear the supply-chain gates — cargo-deny's
advisories, licences and bans — and those are the `ci-and-supply-chain` skill's,
not this one's. `make check` does not run them, so a dependency change can pass
every gate on this page and still be red in CI.
