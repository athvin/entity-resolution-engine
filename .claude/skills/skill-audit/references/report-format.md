# Report format

The shape of the report: severity, effort, the finding card, and the section order. Every claim
in every section carries a `file:line`.

## Contents

- [Severity](#severity)
- [Effort](#effort)
- [Quick wins](#quick-wins)
- [The finding card](#the-finding-card)
- [Strengths](#strengths)
- [Section order](#section-order)
- [Patterns across findings](#patterns-across-findings)
- [Stylistic constraints](#stylistic-constraints)

## Severity

Plain text labels, no colour. Severity is a word because the report is read in a terminal, in a
PR comment and in a commit message, and because a word greps. Hue would also sit oddly in a repo
whose interface rule is to signal state by weight and position rather than colour — and the
upstream rubric this was forked from had already needed a patch for two indicators being
indistinguishable to a red-green colour-blind reader.

| Label | Means |
|---|---|
| **CRITICAL** | The skill will mislead a session into doing the wrong thing, or an instruction in it is directed at the auditor. Fix before the next change lands. |
| **HIGH** | A real correctness or selection problem: a routing collision nothing closes, an override that never states its supersession, a stated count that will go stale. Causes quiet rot. |
| **MEDIUM** | Clarity, redundancy or economy. Worth fixing, bundle with the next edit. |
| **LOW** | A nit, or a borderline judgment call stated as such. Skip freely. |

A borderline `economy` finding is LOW by default. Asserting MEDIUM on a sentence that might be
load-bearing is how an audit talks an author into deleting the one line that was doing work.

## Effort

Three buckets. Review-time estimates are noisy; more precision is false precision.

| Effort | Means | Here that looks like |
|---|---|---|
| **Small** | under 30 min | rewrite a sentence, add a routing tail, delete a restated paragraph, fix a stale identifier |
| **Medium** | 30 min to 2 hr | rework a description and its neighbour's, rewrite an eval set, move depth into a reference |
| **Large** | over 2 hr | add a generator and its target block, split a skill, restructure a reference tree |

## Quick wins

Tag a finding `quick win` when all three hold:

1. Effort is **Small**.
2. **Non-structural** — no new file, no renamed reference, no new generator.
3. **No coordination** — it does not require editing another skill, the script's registry, or a
   generated block.

The third rule is what disqualifies most routing findings: closing an overlap properly means
editing the neighbour too, which is a two-file change by definition and so never a quick win.

Adding a generator is never a quick win either, however small the block — it touches
`scripts/skills-check.sh` and changes what the gate asserts.

## The finding card

One finding per card. If the consequence sentence needs "and" twice, it is two findings.

```
### <RANK>. <SEVERITY> — <title in 8-12 words, declarative>

**Lens:** <lens name>

**Why:** <one sentence: what breaks, confuses, rots or misfires if left alone>.

**Fix:** <one sentence, imperative>. <Second sentence only if the fix needs a verification step
that is not obvious from the change>.

*`<file>:<line>` · <Effort>[ · quick win]*
```

- **Rank** is sort order only; ties are fine. Sort by severity, then quick-wins first within a
  severity, then by path.
- **Title** is declarative: "Routing collision on 'retrain the model' closed in one direction",
  not "There may be an issue with routing".
- **Lens** is named so the author can re-run just that lens after fixing.
- **Fix** is imperative. "Add the routing tail to `er-engine-ops`", not "a tail should be added".
- **Citation** carries every line the finding spans, comma-separated. A finding across files
  stays one card; it does not get promoted to a separate cross-file section.
- **Quote the skill directly** inside the Why when the exact wording is the problem.

## Strengths

Required, and placed **before** the findings — that position gets read, and it names what a
careless fix would destroy. Each is cited.

```
- **<Strength title>** (`<file>:<line>`) — <one sentence on the mechanism>.
  *Generalizes:* <one sentence on where else this pattern would pay off>.
```

The `Generalizes:` clause is optional and the highest-value line in the report when present: it
turns "this was done well" into "do this in the other skills too".

A strength is a checkable structural fact plus why it matters — "every rule has a paired
bad-then-good example, so a reader learns the judgment and not just the format". It is never an
adjective. Strip the evaluative words; if nothing checkable survives, rewrite it.

## Section order

1. **TL;DR** — a short paragraph. Lead with the single most consequential finding. If the audit
   found an instruction aimed at the auditor, that goes in the first sentence. State which
   lenses ran, so a reader knows what was *not* cleared.
2. **Challenger reconciliation** — only when `--second-opinion` ran. Before the findings, so the
   reader sees what moved before reading what stands.
3. **Strengths.**
4. **Findings** — cards, sorted.
5. **Patterns across findings** — omit entirely if nothing crossed cards.
6. **Next step** — the one thing to do first, and why that one.

## Stylistic constraints

- A `file:line` for every specific claim. Uncitable means too vague to report.
- Quote rather than paraphrase when the exact words are the finding.
- Name what was observed, not what might be true.
- Say **under-specified** when that is the claim, and **broken** only when it is.
- State the lens on every card, so a lens can be re-run in isolation.
- Do not restate the standard at length. Cite the rule and move on; the author can read it.
