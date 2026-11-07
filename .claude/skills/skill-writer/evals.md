# evals — skill-writer

Hand-run. Pose each in a **fresh** session, in the stated directory. The point of scenarios 1
and 2 is that the skill *refuses* to build what was asked for; those are the ones that catch a
regression into agreeableness.

## 1. Refuses when the rule is already enforced deterministically

**Query:** `/skill-writer a skill that makes sure nobody ever hand-edits a generated entity artifact`

- MUST recognise that `er-entity verify` inside `make check` already regenerates every artifact
  and fails on a stale committed file, so the rule is enforced rather than advisory.
- MUST conclude that neither a skill nor a hook is needed, and say which existing gate covers it.
- MUST explain the reason in terms of deterministic versus non-deterministic instructions — a
  skill is advisory, the harness is not.
- MUST NOT produce a `SKILL.md` whose body says "NEVER hand-edit a generated file" and call that
  done, nor propose a `PreToolUse` hook duplicating a gate that already fails closed.

## 2. Refuses when the knowledge is discoverable

**Query:** `/skill-writer a skill documenting our directory layout and which test framework we use`

- MUST apply the two-part test (could Claude find it by reading the code, and would it look) and
  conclude that layout and framework are discoverable.
- MUST route the *commands* — the exact test and typecheck invocations — to `CLAUDE.md` rather
  than a skill, and keep it short.
- MUST NOT generate a skill that restates the file tree.

## 3. Builds a side-effecting procedure correctly

**Query:** `/skill-writer we cut releases by running changeset version, writing the changelog, tagging, and pushing the tag so CI publishes`

- MUST set `disable-model-invocation: true` and say why.
- MUST include a stop condition (clean tree / correct branch) as step one.
- MUST NOT set `paths:`, and MUST NOT add `allowed-tools` as though it restricted anything.
- MUST keep the body short and ordered rather than explaining what semantic versioning is.

## 4. Obeys the standard over its own body

**Query:** `/skill-writer add a skill for our webhook retry semantics`

- MUST read `.claude/skills/skill-writer/references/agent-skills.md` before drafting.
- MUST put any count, version or threshold in a generated block with a generator added to
  `scripts/skills-check.sh`, never in prose.
- MUST edit the neighbouring descriptions so routing goes both ways — a new skill that names its
  neighbours while they stay silent loses the trigger to the incumbent.
- MUST run `make skills-check` and let it regenerate the inventory tables rather than editing
  `CLAUDE.md` by hand.
- MUST state plainly that the fresh-session trigger check is still outstanding.

## 5. Routes a review request instead of answering it

**Query:** `/skill-writer review .claude/skills/frontend-testing and tell me what's wrong with it`

- MUST route to `skill-audit` rather than producing its own findings.
- MUST NOT re-derive a parallel set of findings, which would disagree with the audit's.
- MAY apply findings the user supplies from an audit, in severity order.
- MUST ask before acting on a finding the audit itself marked borderline, where the fix is a
  deletion.

## 6. Parameter handling

**Query:** `/skill-writer` with no argument.

- MUST ask for one sentence and stop.
- MUST NOT invent a topic or build a generic example skill.
