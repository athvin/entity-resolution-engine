# Evals: ci-and-supply-chain

Hand-run scenarios. Pose each in a **fresh** session and check the expected
behaviours. Record a pass only if every "must" holds.

## 1. The paths-filter trap

**Query:** "`frontend.yml` runs on every PR even when nothing in `frontend/`
changed. Add a `paths:` filter so it only runs when needed."

Must:

- Refuse, and give the mechanism: three of its jobs are **required checks**, and
  a workflow skipped by a path filter **never reports**, so the required check
  stays pending and blocks the PR forever.
- Point at the `detect` job + `if:` pattern as the correct way to get the same
  saving, because a job skipped by `if:` reports `skipped`, which branch
  protection accepts.

Credit for naming a workflow that legitimately carries a real `paths:` filter
because none of its jobs is required — `images.yml` — and for knowing that
`infra.yml`'s filter was **removed** when `terraform plan` became a required
context, because the two cannot coexist.

## 2. A link error that is really a disk error

**Query:** "The Rust `test` job failed with `linking with cc failed: exit status
1`. Is this a toolchain problem?"

Must:

- Say this usually means the **runner ran out of disk**, because a full
  `rust/target` is tens of gigabytes.
- Point at the disk-reclaim step, which every Rust compile job carries
  (`test-shard`, `lake-e2e`, `ducklake-e2e`) — not at one job alone.

Must not:

- Start debugging linker flags or mold configuration first.

## 3. Overnight cargo-deny failure

**Query:** "cargo-deny is red on my PR and I only changed a doc comment."

Must:

- Say the gate is **time-dependent**: it fails on advisories and yanks
  published since the last green run, so the diff did not cause it.
- Say to read the **job log**, not the diff.
- Prescribe fixing it in **its own small PR**, and that an ignore needs a prose
  justification.

Credit for mentioning that stale `audit-allow.json` entries are reported so the
allowlist cannot accumulate.

## 4. An unmergeable dependabot PR

**Query:** "Dependabot opened a PR bumping `arrow` from 58 to 59 and CI won't
go green. Can you fix the compile errors?"

Must:

- Say `arrow` is pinned to the major the **`duckdb` crate** pulls, so an
  independent bump is unmergeable and the unblocker is upstream.
- Recommend closing the PR rather than fixing it.
- Note that `.github/dependabot.yml` **already excludes** `arrow` from major
  updates, so a PR that appears anyway is an anomaly to close, not a signal the
  exclusion is missing.

Must not:

- Start fixing the compile errors, or relax either pin to make them go away.

## 5. Adding a new gate

**Query:** "I wrote a script that fails when a migration file is missing a
down-migration. Wire it into CI as a required check."

Must:

- Make it report on **every** PR (so it can be required) — i.e. not behind a
  workflow `paths:` filter.
- Insist on **observing it fail** before trusting or requiring it.

Credit for adding a rename-detection exit (non-zero when it examined zero
files), and for noting that a job's **display name** is the context branch
protection matches, so renaming a job breaks the requirement.

## 6. Licence list divergence

**Query:** "I added `MPL-2.0` to `rust/deny.toml`'s allow list. Anything else to
do?"

Must:

- Say `frontend/scripts/check-licenses.mjs` holds a **verbatim copy** as
  `DENY_TOML_ALLOW`, and the two must be changed together or the ecosystems
  silently disagree.
- Note the npm checker is an allowlist, so unparseable expressions fail by
  design.

## 7. A green PR that compiled nothing

The selection scenario: the phrasing names no skill, and the wrong answer is
the reassuring one.

**Query:** "This PR shows every required check satisfied and
`mergeStateStatus` CLEAN. Safe to merge?"

Must:

- Load this skill **without being asked for it**, and not answer from
  `gh pr checks` alone.
- Say that a **failed `detect` job** makes every job gated on it report
  `skipped`, which branch protection accepts as satisfied — so that green can
  mean nothing ran.
- Reach the check-run probe: confirm no check-run whose name starts with
  `detect` concluded `failure` for that head SHA.

Must not:

- Treat CLEAN, or a count of satisfied contexts, as proof the tree was built.
- Suggest requiring the `detect` contexts as the fix.

Credit for noting the aggregator gates are defeated the same way, because
`needs.detect.outputs.code` is empty when `detect` failed, so their own
fail-closed `case` never runs.

## 8. Requiring a matrix leg

**Query:** "Make `plan (core)` and `plan (dev)` required checks so an infra PR
can't merge without a plan."

Must:

- Refuse, with the mechanism: when the job is skipped by its `if:`, GitHub
  reports a single check-run under the literal, uninterpolated
  `plan (${{ matrix.key }})`, so the per-leg contexts never report and a PR
  that skips the tier blocks forever.
- Point at the existing non-matrix **aggregator** — `plan-gate`, whose context
  is `terraform plan` — as the thing already doing this.

Must not:

- Propose adding the contexts to see what happens. This was measured: it left
  the next non-infra PR unmergeable with the legs stuck on "Expected".
