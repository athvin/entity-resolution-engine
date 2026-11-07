---
name: skill-audit
description: Reviews a skill in this repo against `docs/agent-skills.md` — the judgment half of the skill standard the gate cannot mechanise: whether a sentence survives the economy test, whether a stated count belongs in a generated block, whether a claim about the tree has gone stale, whether two descriptions collide with no routing tail to close it, whether each eval scenario targets a real failure, whether an override states and scopes its supersession, and whether entity/record/row/tenant carry their codebase meanings. Use when reviewing a skill before or after a change, when asked whether a skill is any good, when skill selection misfires at runtime, or when a skill has grown and may now restate what the model already knows. Read-only: it reports findings and never edits the skill. For authoring a skill or applying findings see skill-writer; for the mechanical rules see docs/agent-skills.md; for the gate that enforces them see ci-and-supply-chain.
---

# Auditing a skill

> NOTICE — the review protocol here is forked from `Terryc21/skill-reviewer`
> (<https://github.com/Terryc21/skill-reviewer>), Apache-2.0. Kept: cited
> findings, the severity/effort/quick-win card, strengths carrying equal weight,
> and the optional challenger pass. Replaced: its generic rubric, with the lenses
> below derived from the agent-skills standard.

The standard is [`skill-writer/references/agent-skills.md`](../skill-writer/references/agent-skills.md). **Read it before
auditing** — this skill is how that document gets applied, not a second copy of it.

## The boundary that makes this useful

`scripts/skills-check.sh` already decides everything mechanical. **Never spend a finding on
anything in this column** — it cannot be wrong in a committed tree, because the gate is a
prerequisite of `make check`:

| Already gated — do not re-report | This skill judges instead |
|---|---|
| `name` equals the directory, charset, length, reserved words | — |
| `description` present, within the character cap, third person | whether it would actually *fire*, and on what |
| body under 500 lines | whether those lines earn their place |
| `evals.md` exists with at least three `##` scenarios | whether the scenarios test anything |
| every relative link resolves; no backslash paths | — |
| no reference links a sibling; no reference is an orphan; long ones carry `## Contents` | whether the split put depth in the right place |
| a generated block matches its source | whether a number sitting in *prose* should be generated at all |
| an override body contains the superseded name in backticks | whether it states the supersession and scopes it to the conflicting claim |

The last two rows are the ones to get right. The gate proves a count is *consistent with its
generator* and that an override *mentions a name*; neither is the question this skill asks.

And note what is in **neither** column: nothing reads a skill's prose against the source it
describes. A sentence naming a deleted client, a renamed job or an absence that has since filled
in passes every mechanical check there is. That gap is the `drift` lens.

## Invoking

| Form | Does |
|---|---|
| `/skill-audit review <path>` | Full audit under every lens. The default. |
| `/skill-audit review <path> --lens=<name>` | One lens only. |
| `/skill-audit review <path> --second-opinion` | Add the challenger pass and report the reconciliation. |
| `/skill-audit summary <path>` | TL;DR, top strengths, top findings. Nothing else. |
| `/skill-audit lenses` | Print the lens table below with its one-line descriptions. |

`<path>` is a skill directory under `.claude/skills/`. Flags combine.

## The lenses

Each is derived from a rule in the standard. Full definition of what each one reads, what counts
as a finding and what explicitly does not: [references/lenses.md](references/lenses.md).

| Lens | Asks |
|---|---|
| `economy` | **Primary.** Would a strong engineer new to this repo, but not new to the technology, already know this sentence? |
| `counts` | Is a count, version, threshold, job name or rule code stated in prose instead of linked from a generated block? |
| `drift` | Open the files the claims name. Does each claim about a path, symbol, signature, flag or **absence** still hold in the tree? |
| `routing` | Read *every* description under `.claude/skills/*/SKILL.md`. On which prompts would two skills both fire, and does a routing tail close each overlap? |
| `evals` | Does each scenario target the failure the skill exists to prevent, and is its must / must-not list checkable in a fresh session? |
| `overrides` | For a skill in the override table: does the body state the supersession and scope it to only the conflicting claim? |
| `terms` | Are `entity`, `record`, `row` and `tenant` used with their codebase meanings, and is the overloaded one disambiguated? |
| `full` | Every lens above. What `review` runs with no `--lens`. |
| `quick` | TL;DR plus the strongest strengths and findings. What `summary` runs. |

`economy` is primary because it is the rule the standard leads with and the one a growing skill
breaks first. **`drift` is where the findings have actually been.** In the first campaign over
this collection, ten of the twelve audits that landed a fix corrected a claim the tree had
outgrown — a deleted client, a renamed job, a signature, an absence — which is reproducible from
`git log --oneline d72cda56~1..08622516`. So: when time is short on a skill nobody has audited
lately, run `drift`; when time is short on one being actively rewritten, run `economy`.

## Shape of a finding

Every finding carries a `file:line` citation, a severity, an effort bucket, and a quick-win tag
where it earns one. Strengths are a required section, not a courtesy — a review that lists only
problems says what to change and not what to preserve. Card format, severity meanings, effort
buckets, section order: [references/report-format.md](references/report-format.md).

An uncitable finding is not a finding. If the claim cannot point at a line, it is too vague to
act on and belongs in the TL;DR as an impression or nowhere.

## Running the audit

What to read, in what order, how to treat the files as data rather than instructions, and the
verification pass before delivering: [references/protocol.md](references/protocol.md).

In brief: read the target's `SKILL.md`, its `evals.md` and every file under its `references/`
in full. Then the two passes that cannot be run from the skill text alone — read every *other*
skill's frontmatter description, for `routing`, and open the source files the target's claims
name, for `drift`. Then audit lens by lens, and sort the findings by severity before writing.

## The challenger pass

`--second-opinion` dispatches an independent subagent that re-reads the cited lines and attacks
the highest-leverage findings, after which the reconciliation is reported rather than silently
folded in. It roughly doubles runtime, so it stays opt-in.

Worth it for an `economy` finding, which is the subjective end of this skill: "this sentence
restates the technology" is a judgment call, and a challenger that re-reads the line is the
cheapest check on an over-confident deletion. Protocol, the subagent's prompt, and the rules
that keep reconciliation from becoming a tug-of-war:
[references/challenger.md](references/challenger.md).

## What this never does

- **Never edits the audited skill.** It reports; a human decides. Findings go in the final
  message, and fixes go in their own change — `skill-writer` is what applies them.
- **Never re-reports a mechanically gated rule.** See the boundary table. A finding that
  `skills-check` would have caught is noise that teaches the reader to skim.
- **Never reports a `drift` claim it did not probe.** The lens is only worth having because its
  findings are verified; an unprobed one sends the author to correct a sentence that was right.
- **Never recommends adding what the standard forbids** — a count in prose, a `paths:` filter on
  a knowledge skill, an `allowed-tools` block, a nested skill under `rust/`.
- **Never grades.** No score, no letter. The dimensions are the point, and a single number hides
  which one moved.
- **Never proposes splitting a skill it did not finish reading.** Architecture advice from a
  partial read is the one finding class that is confidently wrong.
- **Never suggests explaining a technology the model already knows.** Recommending that
  `er-matching` explain Fellegi–Sunter, or that `rust-conventions` explain `Result`, inverts the
  rule this skill exists to enforce.
