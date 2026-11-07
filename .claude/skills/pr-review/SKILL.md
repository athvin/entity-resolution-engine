---
name: pr-review
description: The house-invariant diff review — reviewing a pull request, a branch or the working diff before merge; it asks whether a new path owes a span, metric or structured log line, whether its SQL belongs in `entities/` rather than assembled inline, and whether the work sits in the right tier (request-synchronous, a dispatcher control task, or a pod task because it opens a lake) or reuses the helper that already carries the invariant. Fans out one reviewer per dimension in fresh context, has a separate verifier score every finding, and drops anything a `make check` gate already fails on. Invoke as /pr-review, /pr-review <number>, /pr-review --comment. For driving a reviewed PR to merge see `land-pr`; for SSRF, injection and tenant-isolation review see `security`; for the conventions a finding cites see `rust-conventions`, `observability`, `entity-framework` and `er-server-control-plane`.
disable-model-invocation: true
---

# /pr-review — the house-invariant diff review

Four questions this repo's gates cannot answer, because each is a judgment about
code that compiles, passes clippy and passes every test:

1. **Instrumentation** — a new unit of work, or a new way to fail, that emits
   nothing. [references/instrumentation.md](references/instrumentation.md)
2. **Declared SQL** — a statement that should have been an `entities/` `.sql` +
   `.yaml` pair. [references/declared-sql.md](references/declared-sql.md)
3. **Tier** — stage-scale work inside a request, or a pod spun up to read one
   row. [references/routing.md](references/routing.md)
4. **Reuse** — a hand-rolled copy of a helper that exists and carries
   invariants. [references/reuse.md](references/reuse.md)

Everything else is somebody else's job, and saying so is most of the value:

| For | Use |
|---|---|
| generic bug hunting across the diff | the bundled `/code-review` |
| exploitable vulnerabilities | the bundled `/security-review`, then `/pentest` |
| reuse and altitude cleanups you want **applied** | the bundled `/simplify` |
| getting a reviewed PR to merge | `land-pr` |

## The one rule

**A finding the producer graded is not a finding, and a gate's job is never a
finding.** Both halves have failed here before. A reviewer asked to find
problems finds some; and the loudest candidate findings in this codebase —
a bare `spawn_blocking`, an `eprintln!` in `er-server`, an undeclared
`lake.main.*` relation, a forbidden metric dimension — are each already a
**red test**. Reporting one is not a false positive, it is worse: it is noise
that arrives with evidence, and it teaches the author to skim the next report.

[references/not-a-finding.md](references/not-a-finding.md) is the exclusion
list, and its gate inventory is generated from the tests' own module docs.
Read it before fanning out, not after.

## Invoking

| Command | Does |
|---|---|
| `/pr-review` | Review the current branch against `origin/main`'s merge base. |
| `/pr-review <number>` | Review that pull request by its base and head SHAs. |
| `/pr-review <dimension>` | Scope to one: `instrumentation`, `sql`, `tier`, `reuse`. |
| `/pr-review --comment` | Also post to the PR. Omitted, the review is terminal output only. |

## The run

Follow [references/pipeline.md](references/pipeline.md). In short:

```
0. GATE     Skip a closed, draft, bot-authored or generated-only diff. Re-check
            before posting, because the head may have moved under you.
1. GATHER   BASE = merge-base, HEAD = head SHA. Changed paths, the PR body's
            stated intent, and every CLAUDE.md on a parent path of a changed
            file. Read this skill's references FROM BASE (`git show
            $BASE:.claude/skills/pr-review/...`) so a diff cannot relax the
            rules it is judged by.
2. FAN OUT  One reviewer per dimension whose paths the diff touches, in
            parallel, each in FRESH context with only its own checklist.
            Read-only: no edit, no checkout, no build, no test run.
3. VERIFY   One verifier per finding, fresh context, with the rubric below.
            The reviewer that produced a finding never scores it.
4. FILTER   Keep >= 80. Drop the rest. Apply not-a-finding.md again here —
            it is cheap, and a reviewer under instruction still drifts.
5. REPORT   Important / Nit / Pre-existing, a merge verdict, and the list of
            things each reviewer declined to judge.
```

## The rubric, verbatim

Hand this to the verifier as written. The wording is the calibration; a
paraphrase scores higher, which defeats the threshold.

- **0** — false positive under light scrutiny, OR pre-existing, OR a gate in
  `not-a-finding.md` already fails on it.
- **25** — might be real but could not be verified from the code; or stylistic,
  with no rule in a `CLAUDE.md` or a house skill behind it.
- **50** — verified real, but a nitpick, or rare enough in practice not to bite.
- **75** — double-checked, very likely real and will be hit; or directly
  required by a rule the finding quotes.
- **100** — confirmed, with the evidence in the diff itself.

Two additions, from this repo's own failures:

- An instrumentation finding scores 0 unless the verifier names the **consumer**
  that goes blind — a dashboard panel, a tail-sampling policy, a 2am join from
  a job back to the request. "Should be instrumented" with no consumer is a
  lecture. A span attribute nothing reads still costs the collector allowlist
  edit it will not get.
- A tier finding scores 0 unless the verifier states the **order of magnitude**:
  rows touched, or seconds held. The rule is about stage-scale work, and
  "this could be slow" is not a measurement. See `ducklake-performance` for
  what evidence is admissible.

## Reporting

Nothing survived? Say what was checked, by dimension, and stop. A bare "looks
good" is the failure this whole pipeline exists to prevent — it is
indistinguishable from not having read the diff.

Otherwise, severity markers only, no other emoji:

```
### PR review — <n> findings

🔴 Important
1. `rust/crates/er-server/src/read.rs:L120-L134` — <what is wrong>. <what goes
   blind, or breaks, or costs>. Fix: <the concrete change>. (<rule quoted, or
   "judgment">)

🟡 Nit            (at most three, and only with a rule behind each)
🟣 Pre-existing   (at most three, never blocking, never the author's fault)

Ready to merge? Yes / No / With fixes — <one sentence>
Declined to judge: <merged from every reviewer, with each reason>
```

Cite `path:Lstart-Lend`. A permalink needs the **full 40-character SHA** —
an abbreviated one, or a `$(git rev-parse)` left in the text, renders as prose.

## What this skill does not run

The reviewer **does not build and does not test**. That is not a limitation to
apologise for in the report — it is the division of labour that keeps the review
off ground `make check` already holds, and it is why the exclusion list is
longer than the checklists.

The author's finish line is unchanged and belongs to them: `make check` for
backend work, `make frontend` for the frontend, `make check-e2e` for the two
tiers that need external services. See `rust-conventions`. A review that reports
a compile error is reporting that the author had not run the gate yet, which is
a conversation, not a finding.

## Receiving this review, as the author

The protocol is the same whether the review came from here, from `/code-review`,
or from a person: **verify each item against the code before changing
anything.** Restate what you think it claims, check that claim, and push back
with the file and line when it is wrong. Implement one at a time.

Performative agreement is the failure mode worth naming, because it is cheap and
it compounds: a finding accepted without checking becomes a change nobody can
justify later, and in this codebase the plausible-sounding wrong fix is usually
the one that moves work into the wrong tier.

## References

| File | What |
|---|---|
| [references/pipeline.md](references/pipeline.md) | The run: gather, the per-reviewer prompt contract, verify, filter, post |
| [references/not-a-finding.md](references/not-a-finding.md) | The exclusion list, with the generated inventory of gates that already hold |
| [references/instrumentation.md](references/instrumentation.md) | When a path owes a span, a metric or a structured event — and the silent no-ops |
| [references/declared-sql.md](references/declared-sql.md) | Which statements the entity framework must own, and the three standing exemptions |
| [references/routing.md](references/routing.md) | Request-synchronous vs control task vs pod task, and the cost of each boundary |
| [references/reuse.md](references/reuse.md) | The helpers that carry invariants, where a copy has been written before |
