---
name: skill-writer
description: Authors and repairs this repo's skills from a free-form brief — triages whether the brief should become a skill at all rather than a CLAUDE.md line or a path-scoped rule or a hook, then writes the description that controls selection, keeps the body inside the progressive-disclosure budget and ends on a check that can fail. Carries the frontmatter and argument-substitution reference that docs/agent-skills.md does not, plus worked templates for each skill shape and the anti-patterns with their observable symptoms. Invoke manually as /skill-writer <brief>. Use when adding a skill, when deciding whether a rule belongs in a skill or a hook, when a description will not trigger, or when applying findings an audit produced. For judging an existing skill rather than changing one see skill-audit; for the standard it is held to and its add-a-skill checklist see docs/agent-skills.md.
disable-model-invocation: true
argument-hint: <what the skill should do, or findings to apply>
---

# /skill-writer

**The brief:** $ARGUMENTS

If the brief is empty, ask for one sentence describing what the skill should carry, and stop
until you have it. Do not guess a topic.

## The collection as it stands

```!
printf 'existing skills: '
ls -d .claude/skills/*/ 2>/dev/null | sed 's|.*/skills/||; s|/$||' | tr '\n' ' '
echo
true
```

That block is a worked example of a trap worth carrying: a non-zero exit from an injected
command **aborts the whole skill invocation**, so every probe ends in `|| true` or a trailing
`true`. Write injection blocks that cannot fail.

The list matters because step 2 has to route the new skill against every name in it.

## Step 0 — the standard wins

[`docs/agent-skills.md`](../../../docs/agent-skills.md) is the standard, and it supersedes
everything below on any point of conflict. **Read it before writing anything** — this skill is
the authoring workflow, not a second copy of the rules.

The gate is `make skills-check` (inside `make check`). It regenerates the inventory tables, so
never hand-edit the skill tables in `CLAUDE.md` or in the standard itself.

Then read one existing skill in the same family end to end — `er-matching` and
`rust-conventions` are the house-style exemplars. Match their shape, not your own taste.

## Step 1 — triage: is a skill even the right artifact?

This is the step that saves the most waste, so do it before drafting. Claude reads the code at
the start of every task and is very good at it, so **anything discoverable from the repo should
not be written down at all.** Apply the two-part test: *could it find this by reading the code,
and would it actually go and look?* Two yeses means write nothing.

For what survives that test, pick the artifact by how it must behave:

| The brief is | Artifact | Why |
|---|---|---|
| Discoverable from the code | nothing | Costs attention on every task and buys nothing |
| Needed on nearly every task (build/test commands, a misleading pattern, a settled decision) | `CLAUDE.md` | Loaded every session, so it must stay short |
| A personal preference, not a team rule | `CLAUDE.local.md`, gitignored | Keeps the shared file clean |
| Only relevant inside one subtree | `.claude/rules/<area>.md` with `paths:` | Loads only when work is actually there |
| A rule that must hold **every** time, no exceptions | **hook** or a deny rule | A skill is advisory; the harness is not |
| Team knowledge or a procedure, needed occasionally | **skill** | Description is cheap; body loads only on use |
| A procedure with side effects (release, deploy, migrate, post) | **skill** + `disable-model-invocation: true` | Only a human should start it |
| A self-contained investigation whose transcript you do not want | **skill** + `context: fork` | Runs in a subagent, returns the conclusion |

The hook row is the one most often got wrong. Every instruction is either
**non-deterministic** — a prompt, `CLAUDE.md`, a skill, which the model reads and *decides*
what to do with — or **deterministic**, which the harness executes regardless. Escalating
wording (`IMPORTANT`, `ALWAYS`, bold caps) is the symptom that a non-deterministic artifact is
being asked to do a deterministic job. If the brief contains "always" or "never" and a
violation would be a real incident, the answer is a hook, and a skill alongside it is at best
documentation. Say so plainly and build the hook instead.

The reverse also holds: a hook enforcing a mere preference should be deleted and left to Claude.

If the triage says "not a skill", report that with the recommended artifact and stop. Do not
build a skill to be agreeable.

## Step 2 — write the description before the body

The description is the whole interface. For a model-invocable skill it is the only thing in
context until the skill fires, so it decides whether the right skill is ever selected; for a
user-invoked one it is what a human scans in the `/` menu. Write it first — a description you
cannot write crisply means the scope is wrong.

Requirements:

- **Third person**, stating both **what** it covers **and when** to use it.
- Carry the **trigger vocabulary**: the nouns, commands, filenames and literal error strings
  someone would actually type. "Explains the deploy process" triggers on nothing.
- End with **negative routing** wherever a neighbouring skill is adjacent: *"For X see
  `other-skill`."* Overlapping triggers, not metadata size, are the real cost of a large
  collection.
- Keep it inside the listing budget (`description` plus `when_to_use` is truncated in the skill
  listing, around 1.5k characters; some repos impose a tighter cap — check Step 0).
- If the new skill **supersedes** a general-purpose one, name that skill **in backticks** in
  the body too, so a loaded session is told which claim to discard. Load order alone does not
  tell the model anything.

Routing goes **both ways**. A new description that names its neighbours while they stay silent
about it loses the trigger to whichever neighbour was already answering those questions. Adding
a skill to a family means editing the family — do that edit now, not later.

## Step 3 — body: standing instructions, inside the budget

Keep `SKILL.md` **under 500 lines**, and aim for 100–200. The rendered body enters the
conversation as a message that *persists across turns*, so write **standing instructions**
("when the gate fails, read the job log") rather than one-time steps narrating a single run.

- State **what to do**, not how the technology works. Never re-explain Rust, React, SQL or git
  — the model knows them, and a token spent there is taken from the task.
- Prefer a **feedback loop** (edit → run the check → read the failure → fix → repeat) over
  prose, and a copyable checklist for anything multi-step.
- Push depth into `references/*.md`, **exactly one level deep**: every reference is linked from
  `SKILL.md`, and no reference links to a sibling reference. A chain yields partial reads, and a
  reference nothing links to is never loaded and rots invisibly. To point at a sibling, name it
  in prose.
- Give any reference that could pass ~100 lines a `## Contents` table of contents, so a partial
  read still reveals its full scope. Generated references grow on their own — give them one from
  the start.
- **State no counts.** Counts, version numbers, thresholds and job names go stale within weeks.
  Put them in a generated block with a generator, or leave them out and link to the source. A
  derived *set difference* ("documented but not implemented") is better than a list, because
  nobody has to remember to update it.
- Avoid time-sensitive phrasing. Superseded behaviour goes in an explicit "old patterns"
  section, not in dated prose.
- Pick one term per concept and keep it.

[`references/frontmatter.md`](references/frontmatter.md) — every frontmatter field, argument
substitution, dynamic `!` injection, `context: fork`, invocation control, precedence and naming.

[`references/patterns.md`](references/patterns.md) — worked skills of each kind, description
rewrites before/after, and the anti-patterns with their symptoms.

## Step 4 — frontmatter

Set only what the skill needs. `name` must equal the directory name; lowercase, hyphens.

- `disable-model-invocation: true` for anything side-effecting, and for anything a human should
  deliberately start. Claude then cannot auto-invoke it, cannot preload it into subagents, and
  `/name` still works.
- `user-invocable: false` for the opposite case: background knowledge only Claude should reach
  for, hidden from the `/` menu.
- Free-form prompt parameter → `$ARGUMENTS` in the body plus an `argument-hint`. Reach for named
  `arguments:` only for fixed positional slots, since those split on whitespace and force the
  caller to quote.
- Skip `allowed-tools` unless a prompt is genuinely in the way: it only *pre-approves* for the
  invocation turn and cannot restrict anything.
- Skip `paths:` on a skill meant to answer questions. It gates automatic activation by glob, so
  "how does X work?" fails to trigger when no matching file happens to be open.

Full table in [`references/frontmatter.md`](references/frontmatter.md).

## Step 5 — evals

Ship `evals.md` with at least three scenarios. A scenario is a query plus the behaviours that
**must** and **must not** follow. Write them against the failure the skill exists to prevent,
not against trivia it happens to contain. If the skill supersedes a general-purpose one, make
one scenario the prompt that other skill would answer differently.

They are hand-run: pose each in a **fresh** session. The signal that matters most is whether the
right skill triggers at all — and that an adjacent one does not.

## Step 6 — give it a check, then try to break it

A skill whose procedure ends in a command that can come back pass or fail is worth several that
end in prose, because the loop closes without a human in it. Name the real gate in the body —
the exact command, not "run the tests". For a backend skill that is `make check`, not the bare
cargo trio; for a frontend one, `make frontend`.

Then verify this skill:

```
- [ ] 1. Triage recorded: this is a skill, not CLAUDE.md / a rule / a hook.
- [ ] 2. name == directory name; frontmatter parses; opening `---` is line 1.
- [ ] 3. Description is third person, what + when, carries trigger words, ends in routing.
- [ ] 4. Neighbours' descriptions edited to name this skill back.
- [ ] 5. Body under 500 lines; no restatement of the technology; no counts.
- [ ] 6. Every references/ file linked from SKILL.md; none links to a sibling; links resolve.
- [ ] 7. Any injected `!` block cannot exit non-zero.
- [ ] 8. evals.md has three scenarios with must / must-not behaviours.
- [ ] 9. `make skills-check` green; inventory tables regenerated, never hand-edited.
- [ ] 10. Gate broken on purpose once, and observed failing with a useful message.
- [ ] 11. Eval scenarios posed in a fresh session; right skill fires, adjacent one does not.
```

Steps 1–9 only make the skill well-formed. Steps 10 and 11 are the ones that measure it — an
unobserved gate is not known to work, and a skill that never triggers is inert no matter how
well written.

Report what you built, where, and which of 10 and 11 remain for the user to run by hand.

## Repairing rather than authoring

**Judging an existing skill is `skill-audit`'s job, not this one's.** That skill is read-only by
design and produces cited findings with a severity and an effort; this one edits. Running the
audit first and applying its findings here is the intended pair — do not re-derive the findings,
since the two would disagree and only one of them read the whole tree.

When the brief carries audit findings, apply them in severity order, and treat the audit's
"not a finding" lists as binding: a sentence it declined to flag was declined for a reason. One
exception needs a decision rather than an edit — a finding that recommends **deleting** a
sentence is the irreversible direction, so when the audit itself called it borderline, ask before
cutting.

Then re-run `make skills-check`, and re-pose any eval scenario whose subject the repair touched.
