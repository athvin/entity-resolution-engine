# Agent skills — the standard

How this repo's Claude Code skills are organised, what belongs in one, and how
the gate keeps them from rotting.

The skills themselves live in `.claude/skills/`. This document is the standard
they are held to; `scripts/skills-check.sh` enforces the mechanical half of it.

## Why skills at all

A skill carries what a competent agent **cannot** know: this repo's
conventions, invariants, generated artifacts and commands. It should not
explain Rust, SQL, React or Terraform — the model already knows those, and a
token spent re-explaining them is a token taken from the actual task.

The practical test for any sentence in a skill: *would a strong engineer new to
this repo, but not new to the technology, already know this?* If yes, delete
it.

## The inventory

Fifteen skills, all at the repository root. Organised by **what an engineer is
doing**, not by crate or package.

| Skill | Covers |
|---|---|
| `entity-framework` | the YAML-driven schema and SQL governance; adding a table, column or statement |
| `sql-analysis` | `er-sql-check`: the E0xx diagnostics, rewrites, `mode: partial`, the ff-analysis planner |
| `er-matching` | Fellegi–Sunter: blocking, comparisons, training, frozen TF, the named invariants |
| `er-engine-ops` | driving the pipeline with the `er` CLI; exit codes; correction versus rerun |
| `ducklake-lakehouse` | attaching a lake, layout levers, maintenance ordering, tenancy as shipped |
| `ducklake-performance` | the measurement discipline and storage-tuning rules |
| `er-server-control-plane` | routes, auth lattice, the Postgres queue, the dispatcher, error bodies |
| `api-contract` | utoipa → `openapi.json` → `schema.d.ts`, and what is breaking |
| `rust-conventions` | `make check` as the finish line, the error Display rule, lint policy, pins |
| `frontend-bff` | the BFF boundary, authz in the handler, no `NEXT_PUBLIC_*`, the monochrome rule |
| `frontend-testing` | the four tiers, the coverage floor, axe, PGlite, the Playwright pin |
| `infra-platform` | Terraform layout, EKS Auto Mode, Pod Identity, the Helm charts, IAM debt |
| `ci-and-supply-chain` | the `detect` pattern, required checks, cargo-deny and pnpm audit |
| `security` | the attack surface, the ER-specific vulnerability classes, the harness map |
| `pentest` | the autonomous campaign (user-invoked only) |

## Layout: why everything is at the root

Claude Code discovers skills in nested `.claude/skills/` directories too, and a
monorepo makes that tempting. Two facts rule it out here:

1. **`rust/.claude/` is gitignored in full** and symposium regenerates it
   (`make symposium`). A skill authored there would be untracked and
   eventually wiped.
2. **Nested skills load lazily** — only once the agent reads or edits a file in
   that subtree. A root skill is registered at startup, so it is available as a
   slash command and as a trigger from the first message.

So: one flat directory, and no `paths:` frontmatter. `paths:` would gate
*automatic* activation by glob, which sounds right for a monorepo but means a
question like "how does the BFF boundary work?" fails to trigger when no
matching file happens to be open. Precise descriptions with explicit negative
routing do the job better.

## Authoring rules

These follow Anthropic's skill-authoring guidance; the ones marked **(gated)**
are enforced by `scripts/skills-check.sh lint`.

**Frontmatter**

- `name` **(gated)**: must equal the directory name, lowercase letters, digits
  and hyphens, ≤64 characters, and must not contain reserved words.
- `description` **(gated)**: ≤1,024 characters, **third person**, and must state
  both **what** the skill covers **and when** to use it. This single field is
  how the right skill gets selected out of fifteen, so it carries the trigger
  vocabulary — the nouns and error strings someone would actually type.
- End the description with **negative routing** where two skills are adjacent:
  "For X see *other-skill*." Overlapping triggers are the real cost of a large
  skill collection, not the metadata.
- No `allowed-tools`: it only *pre-approves* tools, it cannot restrict them, and
  nothing here needs pre-approval.
- `disable-model-invocation: true` only for a side-effecting procedure. Today
  that is `pentest` alone.

**Body**

- **Under 500 lines** **(gated)**. In practice ours land near 150.
- **References exactly one level deep** **(gated)**: every `references/*.md`
  links from `SKILL.md`, and a reference must not link to a sibling reference.
  Claude partially reads files reached through another reference, so a chain
  produces incomplete information.
- A reference over 100 lines needs a `## Contents` table of contents **(gated)**,
  so a partial read still reveals the full scope.
- Every relative link must resolve **(gated)**, and paths use forward slashes
  **(gated)**.
- **State no counts.** See below.
- Prefer a workflow with a feedback loop (edit → run the gate → fix → repeat)
  over prose, and a copyable checklist for anything multi-step.
- Avoid time-sensitive phrasing. Put superseded behaviour in an explicit "old
  patterns" or "in flight" section rather than dating the prose.
- Pick one term and keep it. In an entity-resolution codebase, "entity",
  "record" and "row" are three different things.

**Evals**

- Every skill ships `evals.md` with at least three scenarios **(gated)**.
- A scenario is a query plus the behaviours that must and must not follow.
  Write them against the failure the skill exists to prevent, not against
  trivia.
- They are **hand-run**: pose each in a fresh session and check. There is no
  automated runner, by choice — the valuable signal is whether the right skill
  triggers at all, which a scripted harness tends to bypass.

## No skill states a count

`entity-framework/SKILL.md` said "25 entities" while the tree held 30. The
frontend version numbers in an early draft of these skills were wrong within
days. The coverage floor and the advisory ignore list move on their own
schedules.

So: counts, version numbers, job names, rule codes and thresholds live in
**generated** reference files, and prose links to them.

| Generated file | Derived from |
|---|---|
| `entity-framework/references/entities-index.md` | `entities/*/entity.yaml` |
| `rust-conventions/references/crate-map.md` | the crate tree and `rust/Cargo.toml` |
| `sql-analysis/references/diagnostics.md` | `er-sql-check/src/analyze.rs` vs the design doc's table |
| `ci-and-supply-chain/references/required-checks.md` | the workflow files and `rust/deny.toml` |
| `frontend-testing/references/tiers-and-thresholds.md` | `vitest.config.ts`, `package.json`, `frontend.yml` |

Each has a `<!-- BEGIN GENERATED: id -->` / `<!-- END GENERATED: id -->` pair.
Prose outside the markers survives regeneration; **do not hand-edit inside
them**.

The `diagnostics.md` generator is the pattern worth copying: it derives the
"documented but not implemented" list by set-differencing the design doc's table
against the codes actually present in the source. Nobody maintains that list,
so nobody can forget to.

## The gate

```sh
make skills-check          # both modes
bash scripts/skills-check.sh lint    # authoring rules, no toolchain
bash scripts/skills-check.sh regen   # rebuild generated blocks
bash scripts/skills-check.sh check   # regen + fail on drift
```

`make skills-check` is a prerequisite of `make check`.

**The two modes are split by what their output derives from, and the CI
placement follows that split:**

- `regen` output derives from **code**, so it can only go stale when code
  changes — and a code change always runs the full backend gate. It therefore
  lives as a step in the existing `lint` job.
- `lint` output derives from the **skills themselves**, so it must run on a PR
  that touches nothing but `.claude/**`. That is precisely the PR that
  `rust.yml`'s `detect` job skips every compile job for. So `lint` runs in its
  own `skills` job that is **deliberately not gated on `detect`** — it needs no
  toolchain, so running it unconditionally costs seconds.

Getting that backwards would produce a gate that is green exactly when it
matters.

The drift check diffs **only the generated files**, relative to `HEAD`. Diffing
the whole skills tree would report an author's in-progress prose as generated
staleness; being HEAD-relative means a `git add` cannot launder a stale file
past the gate.

The `skills` job is not yet a required status check. Per
`docs/frontend-testing.md`'s rule, promote it only after it has been observed
failing.

## Third-party skills

**We author first-party skills only.** Nothing is vendored and no skill
plugins are installed. Claude already knows Terraform, React, DuckDB and
Playwright generically; what it cannot know is how *this* repo uses them, and
that is the only thing worth the context.

The exception is the symposium-synced set under `rust/.claude/skills/`
(`make symposium`), which is gitignored, regenerable and not ours. One of them,
`rust-best-practice`, declares itself critical and asks to be activated before
any Rust work, and prescribes a finish line of `cargo fmt` + `cargo clippy` +
`cargo test`. **In this repo that checklist is incomplete** — it skips
`entities-check`. `rust-conventions` states the override explicitly, and wins
structurally because root skills load at startup while nested ones load lazily.

If it ever misfires anyway, the escalation is `skillOverrides` in
`.claude/settings.json`. We have not needed it: the rest of that skill's
content is sound, and its companion error-handling skills actually agree with
our own Display rule.

## Adding a skill

```
- [ ] 1. Is this knowledge Claude lacks, or is it a restatement of a technology?
- [ ] 2. Does an existing skill cover it? Prefer extending one over adding a
         sixteenth — overlapping triggers are the real cost.
- [ ] 3. Create .claude/skills/<name>/SKILL.md with name == directory name and a
         third-person what+when description ending in negative routing.
- [ ] 4. Keep the body under 500 lines; push depth into references/ one level deep.
- [ ] 5. Put any count, version or threshold in a GENERATED block and add a
         generator + target to scripts/skills-check.sh.
- [ ] 6. Write evals.md with three scenarios aimed at real failure modes.
- [ ] 7. make skills-check
- [ ] 8. Break it on purpose and confirm the gate fails with a useful message.
- [ ] 9. Add it to the table above and to CLAUDE.md's skill list.
- [ ] 10. Pose the three eval scenarios in a fresh session and confirm the right
          skill triggers — and that an adjacent one does not.
```

Step 10 is the one that actually measures the skill. Steps 1–9 only make it
well-formed.
