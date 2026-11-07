# pr-review — evals

Hand-run. Pose each in a **fresh** session, because the signal that matters most
is which skill fires, and a session that has already loaded one is no test of
that.

This skill is `disable-model-invocation: true`, so scenarios 1–3 are posed as
`/pr-review …`. Scenarios 4–6 are posed as plain prose precisely to check that
it does **not** fire, and that the right neighbour does.

---

## 1. The gate's job is not a finding

**Query:** `/pr-review` on a branch whose diff adds, in `er-server/src`, both a
bare `tokio::task::spawn_blocking` and an `eprintln!("job failed: {e}")`.

**Must:**
- Report neither. Both are red tests — `span_boundaries.rs` and
  `structured_logging.rs` — and the generated inventory in `not-a-finding.md`
  lists them with their one-line claims.
- If anything is said about them at all, it is that `make check` will fail and
  the review is waiting on the author, not a finding.
- State what was checked, by dimension, when nothing else survives.

**Must not:**
- Report either as a finding at any severity, including Nit.
- Run `cargo test`, `make check` or any build to decide.
- Report a dropped-finding count.

**Why this one:** it is the single most likely failure. Both are real defects,
both are easy to spot in a diff, and both arrive with convincing evidence —
which is exactly what makes reporting them expensive.

---

## 2. The tier finding, and its reason

**Query:** `/pr-review` on a branch adding a new `JobKind` whose task body only
reads and writes `task_instances` rows through `er_entities::`.

**Must:**
- Report it: a pod task for Postgres-only work pays the pod overhead and a
  tenant's one-pod slot for milliseconds of SQL. The fix is a control task.
- Give an order of magnitude, from `.claude/skills/er-engine-ops/references/batch-orchestration-design.md` §8
  rather than invented.
- Survive verification with a named consequence, not "this seems heavy".

**Must not:**
- Justify it by latency alone. The deciding question is the writer lock: a
  Postgres-only body needs no lake, and a body that needs a lake open is a pod
  task regardless of speed.
- Claim the control body is structurally impossible to get wrong in the other
  direction — `control_tasks_no_engine.rs` covers linking engine code, nothing
  covers choosing the wrong tier.
- Demand a `REVIEW.md` or any new rules file.

---

## 3. A reviewer does not grade its own work

**Query:** `/pr-review` on any branch with at least one real finding, then ask
"how confident are you in finding 1, and who decided that?"

**Must:**
- Answer that a separate `pr-verifier` in fresh context scored it, with the
  rubric, and that the reviewer which produced it never saw the score.
- Name the read-only tool restriction on both agents as the reason the
  separation is more than an instruction.

**Must not:**
- Re-score the finding itself in the answer, which would collapse the
  separation after the fact.
- Describe the score as the coordinator's judgment.

---

## 4. Adjacent: a generic bug hunt should not land here

**Query:** "Review my working diff for bugs."

**Must:** route to the bundled `/code-review`, and offer `/pr-review` as the
*additional* pass for this repo's four invariants.

**Must not:** auto-fire `pr-review` — it is user-invoked — and must not present
the four house dimensions as a general bug review. A diff can be clean on all
four and still be broken.

---

## 5. Adjacent: "should this be instrumented?" belongs to observability

**Query:** "I'm adding a handler that lists a tenant's imports. What telemetry
does it need?"

**Must:** fire `observability`, which owns the span/metric/event decision, the
registry, and the collector allowlist.

**Must not:** fire `pr-review`. This is an authoring question, not a review of
an existing diff, and `instrumentation.md` deliberately carries only the
owed-or-not judgment rather than a second copy of the emission rules.

---

## 6. Adjacent: driving a PR to merge is `land-pr`

**Query:** "Watch PR #812 and merge it when it's green."

**Must:** fire `land-pr`.

**Must not:** fire `pr-review`, and must not treat a green gate as a review. The
two are sequential, not alternatives: `pr-review` judges the diff, `land-pr`
gets the judged diff onto a green `main`.

---

## What a run of these actually tests

Scenario 1 is the noise floor and is worth re-posing whenever a dimension
checklist grows, because every added rule is a new way to report something the
gates already own. Scenarios 4–6 are the routing, and they are the ones that
break silently when a neighbour's description is edited — the reciprocal clauses
in `land-pr`, `security` and `rust-conventions` are what make them pass, and
nothing fails if someone removes one.
