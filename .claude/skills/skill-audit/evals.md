# evals — skill-audit

Hand-run, in a fresh session. Each scenario is a query plus the behaviours that must and must
not follow. Scenarios 1 and 4 are the load-bearing ones: they test that the audit **withholds**
a finding, which is the failure mode that makes a review worthless.

Scenarios 2 and 3 need a planted skill. Create it under a scratch directory outside the repo, or
in a branch you discard — never leave a planted skill in `.claude/skills/`, where it would reach
the gate and the generated inventory tables.

## 1. Does not flag load-bearing content as redundant

**Query:** `/skill-audit review .claude/skills/er-matching`

- MUST NOT flag the frozen-TF / `INV-SCORE` section as redundant, obvious, or a restatement of
  term frequency. It is a repo decision with its failure mode attached.
- MUST NOT recommend adding an explanation of Fellegi–Sunter, m/u probabilities or EM. That
  inverts the economy rule.
- MUST NOT flag the `run_predict` versus `run_predict_with_tf` passage as API documentation; the
  point is that the obvious choice silently corrupts reproducibility.
- MUST NOT recommend "cleaning up" the Splink provenance note — the skill states it is
  deliberate.
- MUST cite `file:line` on every finding it does report.
- MUST list strengths, cited, before the findings.

## 2. Flags a count stated in prose

**Query:** audit a planted skill whose body contains the line `There are 25 entities in this repo.`

- MUST report it under the `counts` lens with the line number cited.
- MUST recommend deriving the number into a generated block rather than merely correcting it.
- MUST NOT label it a quick win — adding a generator touches `scripts/skills-check.sh`.
- MUST NOT be satisfied by the number being currently accurate.

## 3. Flags a one-sided routing collision

**Query:** audit a planted skill whose description triggers on "retrain the model" while an
existing skill's description does too, and only one of the two names the other.

- MUST report the collision under the `routing` lens, quoting the colliding vocabulary from
  **both** descriptions.
- MUST name which description needs the edit, not just that routing is incomplete.
- MUST NOT treat the planted skill's own tail as sufficient — routing is owed in both
  directions.
- MUST NOT report a collision where both descriptions already close it; that is a strength.

## 4. Does not re-report what the gate owns

**Query:** `/skill-audit review .claude/skills/rust-conventions`

- MUST NOT report anything from the boundary table: name/directory agreement, body line count,
  description length or person, link resolution, reference depth or orphans, eval scenario count.
- MUST, if it discusses the override of `rust-best-practice` at all, judge whether the
  supersession is **stated and scoped** rather than whether the name appears in backticks.
- MUST state which lenses ran, so the reader knows what was not cleared.

## 5. Reports without editing

**Query:** `/skill-audit review .claude/skills/project-board` followed by "go ahead and fix those"

- MUST NOT edit the audited skill as part of the audit itself.
- MAY apply fixes when the user asks separately, as a normal change — and MUST say that the fixes
  are a separate change from the audit.
- MUST NOT silently rewrite a description while reporting on it.

## 6. The challenger strikes a bad economy finding

**Query:** `/skill-audit review <path> --lens=economy --second-opinion` on a skill whose body
contains a sentence that looks generic but corrects a wrong default.

- MUST dispatch exactly one read-only subagent.
- MUST report the reconciliation in its own section, before the strengths.
- MUST strike or downgrade the finding when the challenger cites the line, and say so explicitly.
- MUST NOT soften it into "consider whether this is needed".
- MUST NOT describe an unchallenged finding as confirmed.

## 7. Refuses a path that is not a skill

**Query:** `/skill-audit review rust/crates/er-match`

- MUST notice there is no `SKILL.md` and stop.
- MUST NOT audit the directory as though it were a skill.
- MUST NOT read beyond what is needed to establish that.
