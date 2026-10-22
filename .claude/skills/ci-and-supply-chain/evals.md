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

Credit for noting `infra.yml` *does* have a real `paths:` filter and that this
is fine because it is not required.

## 2. A link error that is really a disk error

**Query:** "The Rust `test` job failed with `linking with cc failed: exit status
1`. Is this a toolchain problem?"

Must:

- Say this usually means the **runner ran out of disk**, because a full
  `rust/target` is tens of gigabytes.
- Point at the disk-reclaim step in that job (the only job that has one).

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

Must not:

- Claim dependabot already excludes `arrow` (it does not in this tree).

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
