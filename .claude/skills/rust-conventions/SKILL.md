---
name: rust-conventions
description: The authoritative Rust conventions for this workspace — that `make check` (not the bare cargo trio) is the finish line, the enforced rule that an error's Display must never interpolate its own source and how to flatten a chain instead, the workspace lint policy and why each clippy allow exists, the MSRV and edition-2024 consequences, and the TLS/AWS feature pins that must not be relaxed. Use before authoring or reviewing any Rust in this repo, when deciding what to run before calling backend work done, when adding a dependency or a feature, when writing an error type, or when a clippy or doc warning fails the build. This supersedes any generic Rust skill's finish-line checklist. For schema or SQL changes see entity-framework; for the control plane see er-server-control-plane.
---

# Rust in this workspace

The cargo workspace root is **`rust/`**, not the repo root. Bare `cargo`
commands must run from there, which is why every Makefile target does `cd rust`
first.

## The finish line is `make check`

```
make check   # = fmt + lint + entities-check + test
```

Run it from the **repo root** before calling backend work done. It is the
PR-equivalent gate and mirrors the `rust` workflow's lint and test tiers:

| Step | Command |
|---|---|
| `fmt` | `cargo fmt --all --check` |
| `lint` | `cargo clippy --workspace --all-targets -- -D warnings` |
| `entities-check` | `cargo run -q -p er-entity-cli -- verify --root ..` |
| `test` | `cargo test --workspace` |

**`cargo fmt` + `cargo clippy` + `cargo test` on their own are not
sufficient.** They skip `entities-check`, the entity-framework gate that
regenerates every artifact from `entities/*.yaml`, fails on a stale committed
file, and runs the E0xx statement passes. A change can pass clippy and every
test and still break that gate.

> **Precedence.** Some installed third-party skills — notably
> `rust-best-practice`, which symposium syncs into `rust/.claude/skills/` —
> prescribe a generic finish line of `cargo fmt` + `cargo clippy --all
> --workspace` + `cargo test --all --workspace`, and that skill asks to be
> activated before any Rust work. In this repo that checklist is **incomplete**.
> This skill supersedes it on the finish line. Its other guidance (and the
> error-handling skills it ships alongside) is fine and does not conflict —
> `library-errors` in particular agrees with the Display rule below.

Two gaps in `make check` worth knowing, because CI catches them and your laptop
does not:

- **`cargo doc` is not in `make check`** but the CI lint job runs it with
  `RUSTDOCFLAGS=-D warnings`. A broken intra-doc link fails CI after a green
  local run. Run `cargo doc --workspace --no-deps` when you have touched doc
  comments.
- Postgres- and DuckLake-gated tests self-skip without their env vars. See
  [references/gates.md](references/gates.md).

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

Each allow exists for a stated reason, and the reasons are load-bearing rather
than concessions — several protect correctness:

| Allowed | Why |
|---|---|
| `cast_possible_truncation`, `cast_precision_loss`, `cast_possible_wrap`, `cast_sign_loss` | numeric engine; i64/usize/f64 conversions are pervasive and intentional |
| `float_cmp` | trained and fixed float parameters **are** compared for exact equality, deliberately |
| `unreadable_literal` | determinism-critical float/hash constants; reformatting risks a transcription error for zero functional gain |
| `doc_markdown` | doc prose names DuckDB, DuckLake, SQL — not code spans |
| `missing_errors_doc`, `missing_panics_doc` | errors and panics are documented in prose |
| `needless_pass_by_value` | CLI handlers own their parsed config; borrowing would add noise |
| `must_use_candidate`, `module_name_repetitions`, `too_many_lines`, `items_after_statements`, `struct_excessive_bools` | style preferences that fight the codebase's shape |

Do not add a new `#[allow]` at a call site to silence a pedantic lint without
saying why in a comment — the workspace table sets the precedent that an allow
carries its reason.

## MSRV, edition, and what follows from them

- **`rust-version = "1.88"`**, `edition = "2024"`.
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

A PR that bumps either alone can never go green, and the unblocker is upstream.
Note that `.github/dependabot.yml` does **not** currently exclude them, so such
PRs will still be opened — recognise and close them rather than trying to make
them pass. (`@playwright/test` *is* excluded, for the image-tag reason the
frontend-testing skill explains.)
`duckdb` itself is an **`=` pin** so that a DuckDB change is always a
deliberate, reviewable event — it can move emitted SQL and floating-point
output, which the goldens compare.

The current crate inventory and version pins are generated, not written here:
[references/crate-map.md](references/crate-map.md).
