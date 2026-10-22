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

All at the repository root, organised by **what an engineer is doing**, not by
crate or package.

The table is **generated** from each `SKILL.md`'s frontmatter by
`scripts/skills-check.sh regen` — the name from `name`, the summary from the
first em-dash clause of `description`, and the user-invoked marker from
`disable-model-invocation`. Do not hand-edit it, and do not state a count
beside it. This table and the one in `CLAUDE.md` were the two places a human had
to remember to edit when adding a skill, and both said "Fifteen" while the tree
held more. That is the same defect as a stale count inside a skill, so it gets
the same fix.

<!-- BEGIN GENERATED: skill-inventory -->
| Skill | Covers |
|---|---|
| `api-contract` | the utoipa `ApiDoc` whose component schemas come from the generated entity structs |
| `aws-account-operations` | the single-account model where environments are separated by name prefix and permission set rather than by account |
| `ci-and-supply-chain` | the `detect`-job pattern that lets a required check skip without blocking a PR |
| `ducklake-lakehouse` | the ATTACH options and why each is pinned, the native-temp-catalog rule for intermediates |
| `ducklake-performance` | how to read span and resource evidence without over-claiming |
| `entity-framework` | the YAML-driven schema and SQL governance where every table is an entity.yaml |
| `er-engine-ops` | the stage chain (ingest, standardize, match, reconcile, golden) |
| `er-matching` | blocking rules, comparison levels, m/u probabilities, match weights |
| `er-server-control-plane` | how routes and the outer path normalizer are wired in axum, the bearer/API-key auth model and its 404-on-cross-org versus 403-on-role rules |
| `frontend-bff` | that the browser never reaches the Rust control plane, the split between the server-only upstream client and the browser's same-origin client |
| `frontend-data-ui` | the charting policy that every chart is hand-rolled DOM/CSS rather than a library or canvas and why |
| `frontend-design-system` | the Tailwind v4 CSS-first tokens in `globals.css` and the two deliberate WCAG deviations from stock shadcn |
| `frontend-testing` | which tier a change belongs in (unit, component, Playwright functional, the dormant full-stack tier) |
| `infra-platform` | the Terraform env/module layout and shared S3 state with lockfile locking |
| `landing-page` | the `(marketing)` route group that prerenders statically inside the same app that serves the BFF |
| `pentest` | a Strix-style run that does recon over the attack surface, fans out a tester subagent per vulnerability category (user-invoked only) |
| `rust-conventions` | that `make check` (not the bare cargo trio) is the finish line |
| `security` | the attack surface (internet-facing Next.js BFF in front of the privileged Rust er-server control plane) |
| `sql-analysis` | the E0xx diagnostics and their remedies, the rewrite step that strips what the planner cannot parse |
<!-- END GENERATED: skill-inventory -->

A consequence worth knowing when writing a `description`: its first em-dash
clause is load-bearing twice over. It is the trigger vocabulary Claude selects
on, and it is the summary two documents show a human.

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
- **References exactly one level deep** — both halves **(gated)**: every
  `references/*.md` links from `SKILL.md`, and a reference must not link to a
  sibling reference. Claude partially reads files reached through another
  reference, so a chain produces incomplete information; and a reference nothing
  links to is never loaded at all, so it goes stale invisibly while still
  reading like documentation.

  To point at a sibling, name it in prose and say it is reached from the skill
  body — `diagnostics.md` is the worked example ("beside this one, reached from
  the skill body"). All three spellings of a sibling *link* are rejected,
  including the bare filename, which is the one an author is most likely to
  write from inside `references/`.
- A reference over 100 lines needs a `## Contents` table of contents **(gated)**,
  so a partial read still reveals the full scope. Generated content grows, so
  give any reference with a GENERATED block a `## Contents` from the start
  rather than waiting for it to cross the line.
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

### What each generator derives, and from where

Two of the targets live **outside** `.claude/skills/`: the inventory table in
this document and the "Doing → Skill" table in `CLAUDE.md`. Those were the two
places a human had to remember to edit when adding a skill, and both said
"Fifteen" while the tree held more — the same defect as a stale count inside a
skill, so it gets the same fix.

| Generator | Target | Derived from |
|---|---|---|
| `entities-index` | `entity-framework/references/entities-index.md` | `entities/*/entity.yaml` and the declared statement files |
| `crate-map` | `rust-conventions/references/crate-map.md` | the `rust/crates/*` tree, each crate's leading `//!`, and the pinned dependency lines |
| `diagnostics` | `sql-analysis/references/diagnostics.md` | `er-sql-check`'s `analyze.rs` set-differenced against the design doc's table |
| `required-checks` | `ci-and-supply-chain/references/required-checks.md` | every workflow's jobs, names, timeouts and `if:`, plus `deny.toml` |
| `tiers-and-thresholds` | `frontend-testing/references/tiers-and-thresholds.md` | `vitest.config.ts` thresholds, the two Playwright pins, the test directories |
| `design-tokens` | `frontend-design-system/references/tokens-and-primitives.md` | `globals.css` tokens with both themes and their `@theme` reachability, `components.json`, the installed primitives, the palette-ban regex |
| `screens-coverage` | `frontend-data-ui/references/screens.md` | every `src/app/**/page.tsx` route set-differenced against the axe gate's `SCREENS` array |
| `aws-footprint` | `aws-account-operations/references/footprint.md` | `resource "aws_*"` types and counts, the permission sets and whether each has an inline policy, the workspace's `aws-*` crates, the provider pins |
| `skill-inventory` | this document **and** `CLAUDE.md` | every `SKILL.md`'s `name`, `description` and `disable-model-invocation` |

Three of these are **set differences**, and that is the shape worth copying:
`diagnostics`, `screens-coverage` and the inline-policy column of
`aws-footprint` each report a *gap* nobody maintains, so nobody can forget it.
`screens-coverage` replaced a prose warning that a new screen is "otherwise
silently unchecked" — the kind of warning that is always true and never read.

A caution learned writing these: generated content grows, so a reference can
cross the 100-line table-of-contents threshold without anyone editing it. Give
a generated reference a `## Contents` from the start.

## Third-party skills, plugins and MCP servers

**First-party skills remain the default, and the reason is unchanged:** Claude
already knows Terraform, React, DuckDB and Playwright generically; what it
cannot know is how *this* repo uses them, and that is the only thing worth the
context. Nothing below changes that. What it does is name the three narrow
cases where something external earns its place, and record the decision for
every candidate so it is not re-litigated.

### Tier 1 — vendored text

An external rule set copied into one of our own `references/` at a **pinned
commit**, with provenance, licence and a refresh command in the file's
preamble.

Use this when the knowledge is good and general but the delivery is not. The
worked example is `frontend-design-system/references/web-interface-guidelines.md`
(from `vercel-labs/web-interface-guidelines`, MIT): the upstream *skill* is a
thin wrapper that `WebFetch`es its rules at invocation time, which makes every
review a live network dependency that fails offline and in CI, and lets the
instructions an agent follows change with no diff here. A pinned copy is
reviewable and works offline.

The obligation that comes with it: the vendoring preamble must say what our own
gates already cover, so a reviewer does not spend effort re-deriving findings
that the axe gate or ESLint produces on its own.

### Tier 2 — installed plugins

Vendor-maintained only, declared in `.claude/settings.json` via
`enabledPlugins`, with **every bundled skill that contradicts this repo hidden
through `skillOverrides`**.

Hiding is not optional tidying. A plugin's always-on cost is its skill
descriptions, and a skill that is wrong here is worse than absent because it
answers confidently. The AWS toolkit ships CDK, CloudFormation and Amplify
skills against a Terraform-only repo, and SDK skills for languages this repo
does not contain — those are hidden, and `aws-account-operations` states the
Terraform-only rule explicitly so the remainder is safe.

Note that `enabledPlugins` waits for workspace trust, so a fresh clone does not
silently acquire them.

### Tier 3 — MCP servers

Tools, not instructions. An MCP server has no context-poisoning surface of the
kind a `SKILL.md` does, which is what makes it the cheapest tier to accept.

Declared in a checked-in `.mcp.json` and approved through
`enabledMcpjsonServers`, so the configuration is reviewable rather than living
in someone's home directory. Two standing rules: a server that can mutate
infrastructure runs read-only, and **exactly one server per provider** — a
plugin that brings its own duplicate produces competing tools that degrade
agent performance with no error.

A rule learned writing the AWS entry: **do not declare a server whose package
name you have not verified.** `.mcp.json` carries only `aws`, whose invocation
comes verbatim from AWS's own setup documentation. Terraform's server arrives
through its plugin instead, which ships a config its vendor maintains — a
guessed `uvx` package name is a silent failure at session start, and the
failure looks like the server simply not appearing.

### The decision record

| Candidate | Decision | Why |
|---|---|---|
| `web-interface-guidelines` rules | **vendored** | good rules, fetched at runtime upstream |
| `aws-core` plugin | **installed**, 8 of 13 skills hidden | live AWS documentation and the billing/observability/IAM skills; its IaC skills are the wrong tool here |
| `terraform` plugin | **installed** | MCP server only, no always-on skill cost |
| AWS MCP Server | **installed**, read-only | live account introspection, and `retrieve_skill` delivers AWS's own guidance on demand |
| `aws-data-analytics` | out | S3 Tables / Glue / Athena would push toward the wrong architecture; the lake is DuckLake on plain S3 with a Postgres catalog |
| `aws-agents` | out | Bedrock is reached through the Anthropic SDK seam, not AgentCore |
| `aws-dev-toolkit` | out | 34 skills and three MCP servers, overlapping `infra-platform` and `aws-account-operations` |
| `vercel` plugin | out | a large always-on cost for one rule set we vendor instead |
| `impeccable` | out | executes Node scripts and installs hooks; high churn |
| `ui-ux-pro-max` | out | a style catalogue that produces generic recommendations, against a repo whose design is a fixed constraint |
| `marketingskills`, `claude-seo` | deferred | reasonable once a marketing surface exists and copy/SEO work begins |

### Skills we deliberately override

Three general-purpose skills are loaded in a normal session and are each wrong
for this repo in a specific, predictable way. Overriding one is not a one-off;
it is a recurring obligation, and the pattern is always the same — the house
skill states the supersession in its own body, and wins structurally because
root skills load at startup while nested and plugin skills load later.

| Skill | What it gets wrong here | Overridden by |
|---|---|---|
| `rust-best-practice` (symposium-synced, gitignored) | prescribes `cargo fmt` + `cargo clippy` + `cargo test` as the finish line, which **skips `entities-check`** | `rust-conventions` |
| `frontend-design` | asks for a palette of 4–6 named hex values and deliberate aesthetic risk, against a product whose design doc forbids any palette | `frontend-design-system` |
| `dataviz` | assumes a charting library and a colour-bearing palette, against a codebase whose charts are hand-rolled DOM/CSS by decision | `frontend-data-ui` |

In each case the override is partial and says so: `rust-best-practice`'s
error-handling companions agree with our Display rule, `frontend-design`'s
quality floor and writing guidance apply unchanged, and `dataviz`'s form
heuristic is sound. Only the specific conflicting claim is superseded.

If an override ever misfires, the escalation is `skillOverrides` in
`.claude/settings.json` — the same mechanism Tier 2 uses to hide a plugin's
skills.

## Adding a skill

```
- [ ] 1. Is this knowledge Claude lacks, or is it a restatement of a technology?
- [ ] 2. Does an existing skill cover it? Prefer extending one over adding
         another — overlapping triggers are the real cost.
- [ ] 3. Create .claude/skills/<name>/SKILL.md with name == directory name and a
         third-person what+when description ending in negative routing.
- [ ] 4. Keep the body under 500 lines; push depth into references/ one level deep.
         Give any reference a '## Contents' if it could ever pass 100 lines.
- [ ] 5. Put any count, version or threshold in a GENERATED block and add a
         generator + target to scripts/skills-check.sh. Prefer a set difference
         over a list: a derived gap cannot be forgotten.
- [ ] 6. Add routing BOTH ways — the new description names its neighbours, and
         each neighbour's description names it. One-way routing loses to the
         incumbent.
- [ ] 7. Write evals.md with three scenarios aimed at real failure modes. If the
         new skill overrides a general-purpose one, make one scenario the prompt
         that general skill would answer differently.
- [ ] 8. make skills-check   (the inventory tables regenerate themselves)
- [ ] 9. Break it on purpose and confirm the gate fails with a useful message.
- [ ] 10. Pose the three eval scenarios in a fresh session and confirm the right
          skill triggers — and that an adjacent one does not.
```

Step 10 is the one that actually measures the skill. Steps 1–9 only make it
well-formed.

Step 6 is the one most often skipped, and it fails quietly: a new skill that
names its neighbours while they stay silent about it will lose the trigger to
whichever of them was already answering those questions. Adding a skill to a
family means editing the family.
