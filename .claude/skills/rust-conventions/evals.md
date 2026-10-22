# Evals: rust-conventions

Hand-run scenarios. Pose each in a **fresh** session and check the expected
behaviours. Record a pass only if every "must" holds.

Scenario 1 is the one that matters most: it tests whether this skill
successfully out-ranks the symposium-synced `rust-best-practice`, which asks to
be activated before any Rust work and prescribes the wrong finish line.

## 1. The finish line (precedence test)

**Query:** "I've finished the change to `er-golden`. What should I run before
calling it done?"

Must:

- Say **`make check`**, from the repo root.
- Name `entities-check` as the step the bare cargo trio skips, and say why that
  matters (it regenerates from `entities/*.yaml` and fails on stale artifacts).

Must not:

- Offer `cargo fmt` + `cargo clippy --all --workspace` + `cargo test --all
  --workspace` as the complete finish line.

Credit for mentioning that `cargo doc` is **not** in `make check` but is in CI
with `-D warnings`.

## 2. The Display rule, both directions

**Query:** "Add an error variant for a failed config read that includes the
underlying IO error in its message."

Must:

- Write `#[error("reading {path:?}")]` with a `#[from]`/`#[source]` field and
  **not** interpolate it.
- Say it is enforced by a posterity gate that fails the build.

Credit for raising the flip side unprompted: if that error is later flattened
into a `String`, use `errfmt::chain` (or `{e:#}` for anyhow), because
`to_string()` now yields only the outermost message.

## 3. An AWS dependency

**Query:** "Add the AWS SDK for S3 to the workspace so `er-cli` can read a
bucket."

Must:

- Use `cargo add` rather than editing `Cargo.toml`.
- Keep `default-features = false`, and explain the two reasons: defaults pull
  aws-lc-rs, which needs cmake the build image lacks; and `sso` /
  `credentials-process` pull providers the pod cannot use under Pod Identity.
- Put it in the consuming crate's manifest, not `[workspace.dependencies]`.

Credit for noting that `aws sso login` credentials will not work locally as a
result.

## 4. A float comparison clippy allow

**Query:** "clippy is warning about `float_cmp` in the model parameter
comparison. Should I add an epsilon tolerance?"

Must:

- Say `float_cmp` is **already allowed workspace-wide**, deliberately, because
  trained and fixed float parameters are compared for exact equality on
  purpose.
- Not introduce an epsilon that would make a parameter mismatch invisible.

Credit for connecting it to the goldens/parity story — exact equality is the
point.

## 5. A test that needs to set an env var

**Query:** "Write a test that sets `ER_LAKE_CATALOG` and checks the config
reader picks it up."

Must:

- Recognise that edition 2024 makes `std::env::set_var` **`unsafe`**, and
  `unsafe_code = "deny"` is workspace-wide — so the test cannot write process
  env.
- Use an injectable seam instead (a lookup closure, or the thread-local overlay
  in `er-env`).

Must not:

- Propose `unsafe { std::env::set_var(...) }` or an `#[allow(unsafe_code)]`.
