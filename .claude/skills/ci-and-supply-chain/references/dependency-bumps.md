# Dependency bumps and the supply-chain gates

## Contents

- How dependabot is configured, and why
- The pins that cannot move alone
- The Playwright pin moves by hand
- Handling an advisory failure
- Handling a licence failure
- Keeping the two licence lists in step
- Why the batch sizes are what they are

## How dependabot is configured, and why

Three ecosystems, weekly, with **minor and patch grouped into one PR per
ecosystem** and majors deliberately left separate — a major is a migration to
schedule, not a bump to merge on green.

| Ecosystem | Directory | Open-PR limit |
|---|---|---|
| `github-actions` | `/` | default |
| `npm` | `/frontend` | 5 |
| `cargo` | `/rust` | **2** |

`github-actions` exists because every `uses:` is SHA-pinned, and a pinned
action is otherwise never updated. Dependabot resolves each pinned SHA's tag
from the trailing `# vTAG` comment, proposes the next release's SHA, and
rewrites the comment — which is exactly why that comment must be preserved
when bumping by hand.

`npm` and `cargo` are there because nothing else bumped them, and the hole was
real: when npm was added, `next` was several patch releases behind a set of six
advisories, one a high-severity SSRF.

## The pins that cannot move alone

| Pin | Locked to |
|---|---|
| `arrow` | the major the `duckdb` crate pulls |
| `datafusion` (and `datafusion-common`/`-expr` in `er-sql-check`) | the major `ff-analysis` resolves |

These are not preferences. Mismatching them does not produce a version
conflict — it produces a **type mismatch**, because the `LogicalPlan` and arrow
types have to be the same types across the boundary. See the sql-analysis
skill.

Dependabot does **not** currently exclude them, so expect occasional PRs that
cannot pass. Recognise them and close them; the unblocker is upstream
(`duckdb` or feather-flow releasing against the newer major).

`duckdb` itself is an **`=` pin**, so it never moves by accident. A DuckDB
change can alter emitted SQL and floating-point output, which the goldens
compare byte-for-byte — so bumping it is a deliberate task that includes
checking the goldens, not a routine update. It also invalidates the sccache
cache and forces a near-cold rebuild, which is why the compile jobs carry
generous timeouts.

## The Playwright pin moves by hand

`@playwright/test` is **ignored for major and minor updates** by dependabot,
because the version is pinned to the Playwright Docker image tag used by the CI
container and the baseline-refresh script. The committed visual baselines were
rendered in that exact image.

So the package and the image tag move **together, by hand**. Bumping one alone
produces pixel diffs that look like UI regressions and are not — see the
frontend-testing skill.

## Handling an advisory failure

Remember the gate is **time-dependent**: it fails on advisories and yanks
published since the last green run, so the cause is almost never your diff.

1. Read the **job log**. It names the advisory id and the crate or package.
2. Prefer an upgrade. If a patched release exists, take it.
3. If none exists, add an ignore **with a prose justification** — for Rust in
   `rust/deny.toml`'s `ignore` list, for npm in `frontend/audit-allow.json`
   (where a `reason` field is mandatory and the script errors without one).
4. State what makes it non-exploitable here, not just that it is inconvenient.
   The existing entries are the model: an unmaintained-crate notice reached only
   through a specific subtree, or a vulnerability on a code path never called.
5. **Do it in its own small PR.** Entangling a supply-chain fix with a feature
   makes the feature's review about the advisory.

An ignore is meant to be removed when upstream moves. `audit-allow.json`
reports **stale entries that match no current advisory**, specifically so the
list cannot quietly accumulate. If that fires, delete the entry.

## Handling a licence failure

`check-licenses.mjs` is an **allowlist**: an SPDX expression it cannot parse
fails rather than passing, and it only handles simple expressions on purpose.

So a failure means one of:

- A genuinely new licence appeared → decide whether it is acceptable, and if so
  add it with a justification.
- A compound or unusual SPDX expression appeared → it falls through to "not
  allowed" by design. Do not loosen the parser to make it pass.

`cargo-deny`'s licence check works the same way, with a `confidence-threshold`
so a near-match is not silently accepted.

## Keeping the two licence lists in step

`frontend/scripts/check-licenses.mjs` holds `DENY_TOML_ALLOW` — a **verbatim
copy** of `rust/deny.toml`'s `[licenses] allow` list — unioned with
npm-specific additions, each keyed to its own justification.

**Change both together.** The duplication is deliberate (the npm tree needs
extra entries that would be wrong to allow for Rust), but it means a change to
the Rust allowlist that is not mirrored leaves the two ecosystems disagreeing
about what is acceptable, silently. The generated reference beside this file
prints the current list from `deny.toml` so a divergence is at least visible.

## Why the batch sizes are what they are

The cargo limit is **2**, down from five, and the reason is worth knowing
before raising it: each cargo PR carries a different `Cargo.lock`, so the
Rust build cache (keyed on the lockfile) misses and the workspace compiles
near-cold. Five simultaneous near-cold builds starved the runners badly enough
to push the lint job past its timeout — on the dependabot branches **and** on
main.

npm stays at 5 because those PRs do not compile Rust.

Dependabot queues the rest and opens them as these merge, so nothing is lost —
it just stops arriving all at once. A week with several major releases is
exactly when the burst would otherwise happen, since majors are split out.
