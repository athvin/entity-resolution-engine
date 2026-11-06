# The challenger pass

`--second-opinion` dispatches one independent subagent to attack the draft's strongest findings,
then reports the reconciliation. Opt-in: it roughly doubles runtime.

## Contents

- [Why it exists here](#why-it-exists-here)
- [Workflow](#workflow)
- [Reconciliation rules](#reconciliation-rules)
- [When the pass produces nothing usable](#when-the-pass-produces-nothing-usable)
- [Anti-patterns](#anti-patterns)

## Why it exists here

The `economy` lens is the subjective end of this skill. "This sentence restates the technology"
is a judgment, and acting on it **deletes** content — the one recommendation class whose cost is
irreversible and paid by a future session that needed the line. A challenger that independently
re-reads the cited lines is the cheapest available check on an over-confident deletion.

This mirrors the discipline `/pentest` already runs on: a finding is confirmed only when a
separate agent reproduces it in fresh context with read-only tools. Same reasoning, lower
stakes.

Use it when the audit is about to drive edits, when the draft reads suspiciously clean or
suspiciously harsh, or when the draft's strongest finding recommends deleting a section. Skip it
for a routine look at a skill nobody is about to change.

## Workflow

### 1. Finish the draft

Run the full protocol and produce the complete report first, exactly as if the flag were absent.
A half-draft gives the challenger nothing to attack and wastes the pass.

Then name the **three highest-leverage conclusions** — the findings or strengths that most shape
the verdict. These are what the challenger tests. If the draft does not obviously have three,
pick the three whose reversal would change the Next step.

### 2. Dispatch

One subagent, read-only (`Explore` is the right type — it cannot edit, which is the point). It
must read the cited lines itself; the draft's quotes are not evidence to it.

Prompt shape:

```
You are auditing an audit. A skill at <path> was reviewed against docs/agent-skills.md and
the draft report is below. Do NOT redo the review.

Your job is to REFUTE the three conclusions listed as highest-leverage. For each:
1. Read the cited file:line yourself. Do not trust the draft's quote.
2. Decide: agree, partially agree, or refute.
3. To refute, say specifically what the reviewer got wrong, with your own file:line.
4. To partially agree, say what nuance is missing.

Default to refuting when uncertain. Weigh these specifically:
- An `economy` finding recommending a deletion: does the sentence correct a wrong default,
  carry a named invariant, or explain why something that looks like cruft must stay? Any of
  those makes it load-bearing and the finding wrong.
- A `routing` finding: read BOTH descriptions in full. A tail you did not see makes it wrong.
- A `counts` finding: is the number inventory that drifts, or a measurement with a fixture
  behind it?
- Any finding the gate in scripts/skills-check.sh already owns is out of scope and should be
  struck.

Then: did the reviewer miss anything that belongs in the top findings?

Output sections: Agreements, Refutations (with file:line), Partial agreements, Missed findings.
Vague disagreement is worse than none. Cite every claim.

Draft report:
---
<draft>
```

### 3. Reconcile

Add a **Challenger reconciliation** section between the TL;DR and the Strengths, with four
parts: agreements, refutations, refinements, missed findings. Then re-sort the cards and redo
the Next step, since a struck finding may have been the one it pointed at.

## Reconciliation rules

These stop the pass becoming a tug-of-war:

- **Both agree** → the finding stands; note the agreement in one line.
- **Challenger refutes with its own citation that holds** → strike or downgrade the finding, and
  say so. Mark it `(struck after challenge)` or `(downgraded after challenge)`.
- **Challenger refutes without a citation, or misreads the line** → the finding stands. Say
  which line was misread and what it actually says.
- **Both defensible** → keep the finding and label it `(contested)`. An unresolved disagreement
  on a judgment call is information, not a failure.
- **Challenger surfaces a missed finding** → add it as a card, marked `(surfaced in challenge)`,
  and apply the gate boundary check to it like any other.
- **Never delete a finding silently.** A finding that disappears between draft and final must
  appear in the reconciliation with its reason.

An `economy` finding refuted by the challenger should be **struck, not softened**. Softening
produces "consider whether this sentence is needed", which is advice no author can act on.

## When the pass produces nothing usable

If the subagent returns empty, refuses, or disagrees with no citations:

- Keep the section and say the challenge produced no usable findings. Removing it would imply a
  single-reviewer report when two were asked for.
- Do not treat silence as confirmation. An unchallenged finding is unchallenged, not confirmed —
  word the TL;DR accordingly.

## Anti-patterns

- **Dispatching before the draft is complete.** The pass exists to challenge a finished opinion.
- **Letting the challenger rewrite the report.** It produces a challenge; the main session
  integrates it.
- **More than one challenger.** A second adds little; if the first strongly disagrees, report the
  disagreement and let a human break the tie.
- **Giving the challenger write tools.** Read-only is what makes its agreement worth anything.
- **Running it on a `quick` audit.** Challenging a deliberately shallow read tests the shallowness,
  not the skill.
