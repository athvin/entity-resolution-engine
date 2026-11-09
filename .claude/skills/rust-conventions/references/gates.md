# The gates, and what each one actually catches

## Contents

- What `make check` covers
- What it does not cover
- The gated test tiers
- Postgres tests: one per file
- The posterity gates
- Running the right thing for your change

## What `make check` covers

Run from the repo root. It mirrors the `rust` workflow's lint, entities and test
jobs, so a green `make check` means the three biggest CI jobs should pass.

The step list below is **generated from the `check:` target**, because it has
grown twice — `doc` and `skills-check` were both added after this reference was
written, and nothing failed while the prose said otherwise.

<!-- BEGIN GENERATED: rust-gates -->
**`make check` = ci-speed-check + fmt + lint + doc + entities-check + skills-check + agent-bridge-check + scripts-home-check + workflow-shell-check + pr-contract-check + tf-static-credentials-check + test** — the `check:` target's prerequisites, in order.

| Step | Command |
|---|---|
| `ci-speed-check` | `python3 -m unittest discover -s scripts/ci -p 'test_ci_*.py'; bash scripts/ci/detect-paths-test.sh; bash scripts/ci/marketing-ci-test.sh; bash scripts/ci/pass-gate-test.sh` |
| `fmt` | `cd rust && cargo fmt --all --check` |
| `lint` | `cd rust && cargo clippy --workspace --all-targets -- -D warnings` |
| `doc` | `cd rust && RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --document-private-items` |
| `entities-check` | `cd rust && cargo run -q -p er-entity-cli -- verify --root ..` |
| `skills-check` | `bash scripts/ci/skills-check-test.sh && bash scripts/skills-check.sh all` |
| `agent-bridge-check` | `bash scripts/ci/agent-bridge-check-test.sh && bash scripts/ci/agent-bridge-check.sh && bash scripts/ci/codex-mcp-sync-test.sh && bash scripts/ci/codex-hooks-sync-test.sh && bash scripts/ci/kimi-mcp-sync-test.sh && bash scripts/ci/kimi-hooks-sync-test.sh` |
| `scripts-home-check` | `bash scripts/ci/scripts-home-check.sh` |
| `workflow-shell-check` | `bash scripts/ci/workflow-shell-check-test.sh && bash scripts/ci/workflow-shell-check.sh` |
| `pr-contract-check` | `bash scripts/ci/pr-contract-test.sh` |
| `tf-static-credentials-check` | `bash scripts/ci/tf-static-credentials-check-test.sh && bash scripts/ci/tf-static-credentials-check.sh` |
| `test` | `cd rust && cargo test --workspace` |

**58 test files read `ER_TEST_PG_DSN`** and self-skip without it.
<!-- END GENERATED: rust-gates -->

## What it does not cover

Three classes of failure survive a green `make check`:

**1. Postgres-backed tests.** They self-skip without `ER_TEST_PG_DSN`:

```rust
let Ok(dsn) = std::env::var("ER_TEST_PG_DSN") else {
    eprintln!("skipping …"); return;
};
```

Skip, not fail — so `cargo test --workspace` on a laptop with no database
reports success while leaving the whole control-plane surface unexercised. The
size of that tier is in the generated block above, and it is most of the
control-plane coverage there is.

**2. DuckLake and extension tiers.** Gated on `DEDUPE_E2E_DUCKLAKE=1` and
`DEDUPE_E2E_EXTENSIONS=1` (the latter needs network to `INSTALL`). Scale
goldens are gated on `DEDUPE_SCALE_TESTS` and run on main/nightly only.

**3. The frontend.** Entirely separate (`make frontend`), with its own required
checks.

Dependency advisories, licences and bans are not here either — they are a
separate CI job, and the `ci-and-supply-chain` skill owns them.

The practical consequence: **a bug in a Postgres or DuckLake path cannot be
caught by the default test run.** If your change touches `er-server`,
`er-cli`, `er-entity-diff` or the session layer, run the gated tier.

## The gated test tiers

```sh
make check-e2e     # DuckLake pipeline + Postgres control plane
```

which is, expanded:

```sh
cd rust && DEDUPE_E2E_DUCKLAKE=1 DEDUPE_E2E_EXTENSIONS=1 \
  ER_TEST_PG_DSN="<dsn>" \
  cargo test -p dedupe-cli -p er-cli -p er-server -p er-entity-diff
```

| Variable | Gates |
|---|---|
| `ER_TEST_PG_DSN` | every `pg_*.rs` test plus `er-entity-diff`'s live diff |
| `DEDUPE_E2E_DUCKLAKE` | the DuckLake e2e tier (checked as `== Ok("1")`) |
| `DEDUPE_E2E_EXTENSIONS` | community-extension loading; needs network |
| `DEDUPE_SCALE_TESTS` | the 50k scale goldens |

Other `ER_*` variables in tests are fixture inputs, not gates.

## Postgres tests: one per file

**Every `pg_*.rs` integration test must be exactly one `#[tokio::test]`** with
sequential phases inside it. This is enforced by `er-server/tests/pg_serial.rs`.

The reason: cargo runs the tests inside one binary concurrently, and every
Postgres test in the crate shares a single schema that it drops and recreates
on the way in. Two tests in one file are two tests racing on the same tables —
one truncates the world out from under the other, or, worse because it looks
like a product bug, leaves state behind that the other asserts against.

This is not hypothetical; it has happened and was fixed by adding the gate. So
when adding control-plane coverage, extend the existing single test with
another phase rather than adding a second `#[tokio::test]`.

Combined with the self-skip above, the failure mode is nasty: a second test in
a `pg_*.rs` file passes locally (skipped, no DSN), passes the CI `test` job
(also skipped), and only fails in the DuckLake e2e job where a database exists.

## The posterity gates

Structural invariants enforced as tests rather than prose. They fail loudly
and name the offending file, and they are the gates most likely to surprise
someone making an otherwise reasonable change.

| Gate | Asserts |
|---|---|
| `er-errors/tests/display_no_source.rs` | no error `Display` interpolates its own source |
| `er-schema/tests/declared_names.rs` | every `lake.main.<relation>` literal in the workspace names a relation the registry declares |
| `er-schema/tests/freeform_sql.rs` | `Session::query_strings` has no product callers outside the engine, and `er-server/src` names no `lake.main.<x>` at all |
| `er-schema/tests/model_registry.rs` | the registry matches its committed golden |
| `er-server/tests/pg_serial.rs` | one `#[tokio::test]` per `pg_*.rs` file |
| `er-entity-lint/tests/tree_is_clean.rs` | the committed `entities/` tree satisfies every V-rule |
| `er-sql-check/tests/oracle.rs` | DuckDB's real result types match the declarations |

The two `er-schema` SQL gates are the reason a security fix or a quick feature
cannot assemble lake SQL inline in `er-server`: it has to become a declared
statement. The one product exemption is visible in a call *name*
(`query_scratch_strings`) rather than an allowlist, so it cannot be granted
quietly.

## Running the right thing for your change

| Changed | Run |
|---|---|
| ordinary Rust logic | `make check` |
| doc comments | `make check` (its `doc` step denies rustdoc warnings, private-item links included) |
| anything in `entities/` | `make check` (its `entities-check` step is the gate) |
| anything in `.claude/skills/` | `make check` (its `skills-check` step is the gate), or `make skills-check` alone — it needs no toolchain |
| `er-server`, `pg.rs`, the dispatcher | `make check` **and** the Postgres tier |
| the session layer, `er-cli`, lake reads/writes | `make check` **and** `make check-e2e` |
| a new DuckDB extension or comparator | the extension tier too (needs network) |
| dependencies | `make check`, and expect the supply-chain job to have its own opinion — see the ci-and-supply-chain skill |
