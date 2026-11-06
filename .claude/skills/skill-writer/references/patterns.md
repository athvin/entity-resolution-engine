# Worked skills, description rewrites, and anti-patterns

Templates to copy and the failure modes to check against. For the field semantics behind any
frontmatter used here, see the frontmatter reference beside this one, reached from the skill body.

## Contents

- [Pattern: team knowledge](#pattern-team-knowledge)
- [Pattern: side-effecting procedure](#pattern-side-effecting-procedure)
- [Pattern: forked investigation](#pattern-forked-investigation)
- [Pattern: path-scoped rule (not a skill)](#pattern-path-scoped-rule-not-a-skill)
- [Pattern: hook (not a skill)](#pattern-hook-not-a-skill)
- [Descriptions, before and after](#descriptions-before-and-after)
- [Anti-patterns and their symptoms](#anti-patterns-and-their-symptoms)
- [An evals.md worth running](#an-evalsmd-worth-running)

## Pattern: team knowledge

The default shape. Model-invocable, no side effects, triggered by vocabulary.

```markdown
---
name: incident-response
description: Explains how this team runs a production incident — the severity ladder and who
  declares each level, the status-page and customer-comms obligations that are contractual
  rather than optional, where the runbooks live, and the blameless-postmortem template with
  its 48-hour deadline. Use when an alert fires, when deciding whether something is a SEV,
  when writing a postmortem, or when asked who to page. For deploying a fix see `release`;
  for the alert definitions themselves see `observability`.
---

# Running an incident

## Declaring

| Severity | Who declares | Page | Status page |
|---|---|---|---|
| SEV1 | anyone | on-call + eng lead | required, within 15 min |
| SEV2 | on-call | on-call | required if customer-visible |
| SEV3 | on-call | nobody | no |

The rule people get wrong: **the status page is a contractual obligation, not a judgement
call.** If a SEV1 is declared, it is posted, even if the fix is already merged.

## While it is open

1. One incident channel, named `inc-<date>-<slug>`. All decisions land there, not in DMs.
2. The declarer owns comms until explicitly handed over, out loud, in channel.
3. Mitigate before diagnosing. A rollback that restores service beats a correct root cause.

## Closing it

Postmortem within 48 hours using `docs/postmortem-template.md`. Blameless means naming the
system that permitted the error, never the person who made it.
```

Why it works: the description carries typed vocabulary ("SEV", "postmortem", "who to page"),
routes away from two neighbours, and the body holds only what is not derivable from the repo.

## Pattern: side-effecting procedure

User-invoked only. Short, ordered, with a stop condition.

```markdown
---
name: release
description: Cut a new release — bumps the version, writes the changelog entry, tags it, and
  pushes the tag so CI publishes. Invoke manually as /release.
disable-model-invocation: true
---

1. Confirm you are on `main` with a clean tree. If not, stop and ask.
2. Run `pnpm changeset version` and commit the result.
3. Summarise the PRs merged since the last tag into CHANGELOG.md, one line each, user-facing
   changes only.
4. Tag `v<new version>` and push the tag. CI publishes from there — never publish from a
   developer machine.
```

Two things make it safe: `disable-model-invocation` means no turn can start it by accident, and
step 1 is a **stop condition** rather than a step to be worked around.

## Pattern: forked investigation

```markdown
---
name: audit-deps
description: Audit a dependency's blast radius before an upgrade. Invoke as /audit-deps <crate>.
disable-model-invocation: true
argument-hint: <package name>
context: fork
agent: Explore
background: false
---

For $ARGUMENTS, report:

1. Every call site, grouped by crate, with file:line.
2. Which uses touch the public API versus internals.
3. The pinned version and what pins it (a lockfile, a peer dependency, a comment).
4. The smallest change set that would survive a major bump.

Return findings as a list. Do not edit anything.
```

Forking keeps a large file-reading transcript out of the main conversation. The cost is that the
subagent **cannot see the conversation** — so never fork a skill whose job is to act on what was
just discussed.

## Pattern: path-scoped rule (not a skill)

When the knowledge is a short always-applies rule for one subtree, `.claude/rules/` beats a
skill: it loads only when work is actually there, and it needs no trigger vocabulary.

```markdown
---
paths:
  - "src/api/**/*.ts"
---
- Validate request bodies with the zod schemas in src/api/schemas/, don't hand-roll checks
- Every new endpoint needs an entry in openapi.yaml, CI fails without it
```

## Pattern: hook (not a skill)

When a violation would be an incident, the instruction has to stop being advisory. The tell is
wording escalation: if a `CLAUDE.md` line has acquired `IMPORTANT` or `ALWAYS` and is still
missed, that line wants to be a hook. Likewise, a `Stop` hook running the typecheck is what makes
"typecheck before you finish" true every time rather than most times.

A skill can document the rule, but it cannot be the enforcement. Say which one the brief needs.

## Descriptions, before and after

| Before | Why it fails | After |
|---|---|---|
| `Helps with testing.` | No trigger words, no scope, no when. | Explains this repo's test tiers — which tier a change belongs in, the coverage floor, and the Docker-backed tier that is a release gate rather than a PR gate. Use when adding a test, when a run fails, or when a CI job is red. For CI job wiring see `ci-and-supply-chain`. |
| `Use this skill for all database work.` | "All X work" collides with every neighbour and wins triggers it should lose. | Explains how schema changes are declared here — every table is an entity.yaml and generated artifacts are never hand-edited. Use when adding a table or column, or when `verify` reports a stale generated file. For query planning diagnostics see `sql-analysis`. |
| `IMPORTANT: always read this before writing any code!!` | Emphasis is not trigger vocabulary; it competes with every other skill and informs none. | Split by topic into real descriptions, or move the rule to `CLAUDE.md` / a hook if it truly always applies. |
| `Deploys to production.` | Model-invocable by default, so a turn can start a deploy. | Same text plus `disable-model-invocation: true`. |

## Anti-patterns and their symptoms

| Anti-pattern | Symptom you will actually observe |
|---|---|
| Restating the technology | The body explains how `async` or `git rebase` works; it reads like a tutorial and buys nothing |
| Hard-coded counts, versions, thresholds | "25 entities" while the tree holds 30. Prose drifts from reality within weeks |
| Emphasis escalation | `IMPORTANT`/`ALWAYS` accumulating on a line that is still sometimes missed — it wants a hook |
| Chained references | `SKILL.md` → `a.md` → `b.md`. Files reached through another reference are read partially, so information arrives incomplete |
| Orphan reference | A `references/` file nothing links to. Never loaded, so it rots while still reading like documentation |
| One-way routing | The new skill names its neighbours; they stay silent. The incumbent keeps winning the trigger |
| `paths:` on a knowledge skill | "How does X work?" fails to trigger because no matching file happens to be open |
| `allowed-tools` used as a restriction | It only pre-approves. Everything stays callable; nothing is sandboxed |
| Fork for a conversational skill | The subagent cannot see the discussion and asks for context the user already gave |
| Injected command without `\|\| true` | A missing file makes the probe exit non-zero and the **whole invocation aborts** |
| One-shot narration | Body written as "first I'll look at…". It persists across turns and then reads as stale history |
| Depth before the point | Everything important after line 400, where auto-compaction's per-skill 5,000-token window may drop it |
| A skill that duplicates a neighbour | Both trigger, the model picks one, and which one is effectively arbitrary. Extend the incumbent instead |

## An evals.md worth running

Three scenarios minimum, each a query plus must / must-not behaviours, aimed at the failure the
skill exists to prevent.

```markdown
# evals — incident-response

## 1. Triggers on the symptom, not the skill name
**Query:** "the checkout API is 500ing for everyone, what do I do"
- MUST load this skill without being named.
- MUST tell the user to declare a SEV1 and post the status page.
- MUST NOT start debugging code before naming the mitigation step.

## 2. Does not steal an adjacent trigger
**Query:** "how do I ship the hotfix once it's reviewed?"
- MUST route to `release` and not answer from this skill.

## 3. Holds the rule that is not a judgement call
**Query:** "it's a SEV1 but the fix is already merged, skip the status page?"
- MUST say the status page is still required.
- MUST NOT present it as the user's call.
```

Pose each in a **fresh** session. The thing being measured is selection, not recall — a skill
with perfect content that never fires is worth nothing, and an automated runner that invokes the
skill directly bypasses exactly the signal you need.
