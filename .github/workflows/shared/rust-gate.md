---
# Shared toolchain prelude for agents that need to run the cheap half of the
# Rust gate. Imported, not copied, so there is one place to fix when `rust.yml`
# moves. Lifted from `rust.yml`'s `lint` job, which is the canonical version.
#
# WHAT THIS DELIBERATELY DOES NOT DO: no disk reclaim. `rust.yml` carries that
# step on the `test` job ALONE, because that job links a separate debug binary
# per crate and exceeds the runner's ~14 GB. `lint` — whose commands these
# agents replicate — has no such step and does not need one. It would also be
# actively harmful here: the reclaim removes `/opt/hostedtoolcache`, and unlike
# `rust.yml`'s jobs a gh-aw run uses `actions/setup-node` for its own runtime,
# which reads exactly that cache.
#
# Agents importing this must NOT run `cargo test --workspace` or the e2e suite.
# Those are `rust.yml`'s job: 9 and 15 minutes respectively, versus ~6 for
# everything below. Let CI be the gate for them.

# NOTE FOR CONSUMERS: gh-aw refuses to let an import escalate privilege, so
# every workflow importing this must ALSO declare `id-token: write` in its own
# `permissions:` block. The compiler fails with "Imported workflows require
# permissions that are not granted in the main workflow" if you forget.
permissions:
  id-token: write # OIDC exchange for the sccache S3 role

network:
  allowed:
    - defaults
    - rust

env:
  # These four must match `rust.yml` EXACTLY. sccache keys on the compiler
  # command line, so a single differing flag — dropping `-fuse-ld=mold`, say —
  # silently yields a 0% hit rate against a cache that is otherwise warm, and
  # the agent waits out a cold duckdb C++ build instead. Matching them is the
  # entire reason these runs are affordable.
  CARGO_INCREMENTAL: "0"
  RUSTFLAGS: "-C link-arg=-fuse-ld=mold"
  RUSTC_WRAPPER: sccache
  SCCACHE_BUCKET: er-ci-sccache-797781631727
  SCCACHE_REGION: us-east-2

steps:
  - name: Install stable toolchain
    run: rustup update stable --no-self-update && rustup default stable && rustup component add rustfmt clippy
  - name: Install mold
    # Retried and time-boxed, exactly as in `rust.yml`: an unpinned network
    # dependency that was observed hanging 20+ minutes on two consecutive
    # attempts. `timeout` bounds each attempt so a stuck mirror is retried
    # rather than waited on. Required despite only mattering at link time,
    # because `RUSTFLAGS` above names it and the flag must match `rust.yml`.
    timeout-minutes: 13
    run: |
      for i in 1 2 3; do
        # See rust.yml for the full reasoning: a partial `apt-get update` that
        # reaches only packages.microsoft.com exits 0, leaves mold out of the
        # index, and every retry then Hit:1 the same cache. Clearing the lists
        # makes the retry able to recover; Error-Mode=any makes an unreachable
        # mirror a failure rather than a warning, so the `&&` actually guards.
        sudo rm -rf /var/lib/apt/lists/*
        if timeout 120 sudo apt-get update -o APT::Update::Error-Mode=any \
           && timeout 120 sudo apt-get install -y mold; then
          exit 0
        fi
        echo "mold install attempt $i failed or timed out; retrying"
        sleep 5
      done
      exit 1
  - name: Assume the sccache read role
    # READ, never write, and not conditional on the ref the way `rust.yml` is.
    # An agent is the last thing that should be able to plant object code a
    # later `main` build consumes. `sccache.tf` trusts
    # `repo:athvin/entity-resolution-engine:*`, so this is assumable from
    # `workflow_run` and `schedule` as well as from a pull request.
    uses: aws-actions/configure-aws-credentials@61815dcd50bd041e203e49132bacad1fd04d2708 # v5.1.1
    with:
      aws-region: us-east-2
      role-to-assume: arn:aws:iam::797781631727:role/er-ci-sccache-read
  - uses: mozilla-actions/sccache-action@fc920bf0ec8de6ee65d409111f7ec508035751ba # v0.0.11
  - uses: Swatinem/rust-cache@d1527c8f67ef9af357290475d4af4b0fab8f12e0 # v2
    with:
      workspaces: rust
      shared-key: rust
      prefix-key: "v1-rust"
      key: ${{ hashFiles('rust/Cargo.toml') }}
      # `rust.yml`'s `lint` job on `main` is the single writer of this key. The
      # 10 GB per-repository Actions cache does not fit this workspace twice,
      # and an agent must not be able to evict the entry every PR depends on.
      save-if: false
  - uses: taiki-e/install-action@68a389c788b2cd04016ce8c80ee23b80d2ab3d68 # v2.88.1
    with:
      tool: cargo-deny
---

## Running the Rust gate

A warm `sccache` is already configured. The commands below are the **cheap half**
of `make check` — together about six minutes. Run them from `rust/` unless noted;
the cargo workspace root is `rust/`, not the repository root.

| Command | What it gates | Warm cost |
|---|---|---|
| `cargo fmt --all --check` | formatting | seconds |
| `cargo clippy --workspace --all-targets -- -D warnings` | lints, as errors | ~1m40 |
| `cargo deny check advisories licenses bans sources` | advisories, licences, duplicate versions, crate sources | ~45s |
| `cargo run -q -p er-entity-cli -- verify --root ..` | the entity framework: every generated artifact regenerated and byte-compared, then every SQL statement planned | ~2m50 |
| `bash scripts/skills-check.sh check` (from the repo root) | generated agent-skill references | seconds |

**Do not run `cargo test --workspace`, `make check`, or `make check-e2e`.** They
are 9 and 15 minutes and belong to `rust.yml`, which runs them on the pull
request anyway. If your change needs them to pass, say so and let CI answer.

Two failure shapes to read correctly rather than retry:

- an `ar`/`cc` error inside `libduckdb-sys`, or `linking with cc failed`, is
  **the disk filling** (errno 28), not a compile error
- `cargo deny` failing on a crate your change never touched means a new advisory
  or yank landed; that is not your diff, and the fix belongs in its own PR with
  a prose justification for any new `deny.toml` ignore

Report a blocked network request rather than working around it. The egress
allowlist is `defaults` plus the Rust ecosystem; nothing here should need more.
