---
name: er-matching
description: Explains this repo's Fellegi–Sunter matching engine — blocking rules, comparison levels, m/u probabilities, match weights, EM training, term-frequency adjustment, clustering and incremental scoring — as implemented in dedupe-core, er-match, er-compile and er-reconcile, together with the named invariants (INV-SCORE, INV-EQ, INV-PERM, CONTRADICTION-1) that scores and entity ids must satisfy. Use when changing blocking or comparison config, training or retraining a model, debugging why two records did or did not match, working on term frequency or incremental scoring, touching the `__dedupe__` SQL stages or a `run_predict*` entry point, or when a golden fixture or a `match_scores` expectation fails. For running the pipeline or the `er` CLI see er-engine-ops; for declaring a table or a SQL statement see entity-framework; for whether a change is actually faster, and the measurements a speed claim needs, see ducklake-performance; for why the `__dedupe__` SQL is never analysed see sql-analysis.
---

# The matching engine

Fellegi–Sunter, implemented in Rust over DuckDB. A pair's score is a sum of
per-comparison **match weights** in log2 (bits), turned into a probability at
the end. Agreement on a rare value is worth more than agreement on a common
one, which is what term-frequency adjustment buys.

The layering matters, because almost every mistake is made at a seam:

| Layer | Crate | Owns |
|---|---|---|
| The engine | `dedupe-core` | blocking, comparison vectors, scoring, EM, clustering — generic, no ER concepts |
| The ER stage | `er-match` | model loading and the **frozen-TF** tables built from a `tf_lookup` snapshot |
| Stage orchestration | `er-cli` | the scoring composition, the ensemble loop, the MERGE into `match_scores` |
| Tenant config → model | `er-compile` | a tenant's declared `comparisons:`/`blocking:` → the settings document |
| Entity formation | `er-reconcile` | assertion-adjusted clustering, entity identity, never-cut |

The seam worth knowing about that table: `er-match` is small and does not
score. Its `//!` header still claims the `match_scores` wrapper "lands here",
but the composition and the write are in `er-cli` — so a review of "the
matching stage" that stays inside `er-match` has read the model loader and
nothing else.

## The one trap that silently corrupts scores

**The rule is about where term frequency comes from, not about which function
is called.** On the ER path TF always comes from a frozen `tf_lookup` snapshot.
`dedupe-core` exposes three prediction entry points and two of them recompute
it:

| Entry point | TF source |
|---|---|
| `run_predict` | recomputed from the input it is given |
| `run_predict_chunked` | recomputed likewise — it is `run_predict` in bounded-memory passes, and a `num_chunks` of 1 dispatches straight to it |
| `run_predict_with_tf` | the TF tables the caller supplies |

Frozen TF is **D4/INV-SCORE**:

> `match_probability` is a pure function of `(model_version, tf_snapshot_id,
> rec_a_key, rec_b_key, rec_a_content_hash, rec_b_content_hash)`.

Nothing in that tuple is "the batch". If TF were recomputed per run, the same
pair would score differently depending on which other records happened to be
present — so an incremental batch could never agree with a full run, and
yesterday's score would not reproduce.

**Two committed shapes satisfy it, and a review that knows only one will flag
correct code.** `er-match`'s `build_frozen_tf_tables` produces the tables from
the snapshot; from there

- `preview_match` hands them to `run_predict_with_tf`, and
- the committed match stage (`run_match` → `run_match_ensemble` →
  `score_member_full` → `predict_member`) composes `dedupe-core`'s
  `materialize_concat`, `materialize_blocked_pairs` and `predict_pipeline`
  directly with the same tables.

The second is deliberate and is not a frozen-TF violation: the cut between
blocking and scoring is what lets a `then_run` ensemble member score only the
pairs an earlier member did not already match, and fusing it back into
`run_predict_with_tf` would give back the saving. `er-cli`'s own comment on
`predict_member` says so, and a `lake_e2e` test asserts the member scores
strictly fewer pairs — so "fixing" it to call the fused entry point turns that
test red.

**Reviewing a change to the scoring path, check the TF source first.** A switch
to `run_predict` — or a reach for `run_predict_chunked` to bound memory on a
large corpus — compiles, passes a unit test on one batch, and breaks
reproducibility everywhere.

## The invariants, by name

The specification defines each once and cites it by name everywhere else. Never
restate one from memory — they are load-bearing and precisely worded. Full
statements with their preconditions and failure modes:
[references/invariants.md](references/invariants.md).

| Name | In one line |
|---|---|
| **INV-SCORE** | a score is a pure function of model, TF snapshot, keys and content hashes |
| **INV-EQ** | under five stated preconditions, incremental reconcile equals full reconcile — as a *set-partition of current members*, not as equal ids |
| **INV-PERM** | an unchanged membership group keeps its `entity_id`, mints no id and emits no event |
| **CONTRADICTION-1** | an `always`-closure component containing an `active` `never` pair fails the run outright |

Two consequences people get wrong:

- **INV-EQ compares partitions, not ids.** Entity ids may legitimately differ
  between a full and an incremental universe. Comparing ids is a false failure.
- **INV-EQ is a conditional claim with two known loss vectors** — incremental
  candidate generation (which can never pair two records that were both already
  in the corpus) and corpus-dependent term frequency. `er correct` exists to
  repair exactly those two, and nothing else.

## The SQL pipeline

The engine generates SQL; DuckDB executes it. Intermediates are named
`__dedupe__*` and live in the **native temp catalog**, never in the lake —
DuckLake cannot host temp tables. Inputs are read from the attached `lake`
catalog by cross-catalog SELECT; final outputs are written back in dedicated
single-catalog transactions.

**Blocking forks off the bare concat; the TF join feeds scoring only.** The
emitted SQL self-joins `__dedupe__concat` to produce candidate pairs, and
`blocked_with_cols` is where those pairs meet `__dedupe__concat_with_tf` — so
no TF value can decide whether a pair exists.

```
__dedupe__concat                      vertical concat of inputs
  ├→ __dedupe__blocked_id_pairs       one UNION ALL arm per blocking rule
  └→ __dedupe__concat_with_tf         TF join (_left/_right variants)
       → blocked_with_cols            the pairs joined to both sides' columns
       → __dedupe__comparison_vectors the gamma CASE columns
       → __dedupe__match_weight_parts per-comparison mw_ / mw_tf_adj_ columns
       → __dedupe__predict            summed weight + match probability
```

Stage-by-stage detail, the clustering and training stage families, and the
cross-rule dedup idiom: [references/pipeline-stages.md](references/pipeline-stages.md).

`__dedupe__*` was renamed from `__splink__df_*`, and this paragraph is the one
statement of what that leaves behind — the references point here rather than
repeat it. Splink survives **deliberately** as provenance: the parity oracle,
the `splink_udfs` extension, `MANIFEST.generated_with`, and historical banners
in the spec. They are the evidence that the Rust rewrite reproduces the
implementation it replaced, so removing them does not tidy the claim, it makes
it unfalsifiable. Do not "clean it up".

## Model identity is never overwritten

A model is a pair of plain files: the settings/parameters JSON and a frozen TF
snapshot CSV. Scores are a function of `(model_version, tf_snapshot_id)`, so a
retrain is a **new identity**, never an in-place edit. Ship them like any other
artifact and pass them to the stage.

Training needs a settings template (the comparison + blocking structure),
deterministic rules for the prior, and EM-session blocking rules. The EM pass
deliberately does not train comparisons consumed by its own training blocking
rule. How each parameter is actually estimated — EM, u by deterministic hash
sampling, m from labels, the prior —
[references/training-and-labels.md](references/training-and-labels.md).

**Scenario tests never train.** EM over `base_10`'s 23 records is degenerate,
so scenarios pin a committed model (`fixtures/static/model_test_v1.json`).

## Tenant config becomes a model through er-compile

A tenant config has always declared `comparisons:` and `blocking:`. Until
`er-compile` existed that declaration was validated at publish, folded into
`config_hash` and rendered in the config studio — and **never read by anything
that ran**. `er-compile` closes that gap by compiling it into a settings
document.

The calibration is the part worth knowing, because the intuition is backwards:
full agreement is scaled to **60 bits**, and *louder is not safer* — past the
calibrated region the model grows confident about pairs that agree on a few
fields, and a false merge costs more than a missed one.

Two scoping facts that decide whether 60 is the right answer to a given
question, both measured in `er-compile/tests/score_scale_calibration.rs`:

- **The band was earned on the committed *person* model**, by two independent
  legs — summing that model's own exact-level Bayes factors, and a cluster-count
  grid search over `base_10`.
- **`compile` hands every config that same scale**, and the committed *account*
  model measures far less full-agreement evidence over its four comparisons. So
  an account config is compiled louder than its own reference model, in the
  expensive direction. That file records it as a measurement rather than a fix,
  and says what closing it needs.

So the scale is a calibrated constant for person configs and an open question
for account ones. The numbers, the grid search and the weight→probability
mapping behind them: [references/config-to-model.md](references/config-to-model.md).

## The goldens are the contract

Changes to the engine are governed by captured fixtures, not by the SQL
analyser — `dedupe-core`'s model SQL is deliberately outside `er-sql-check`'s
remit and under an execution oracle instead.

- Output fixtures per stage, gated by a SHA-256 `MANIFEST.json` so a partial
  regeneration cannot pass unnoticed.
- The **emitted SQL itself**, for the main corpus only: `fake_1000` carries one
  file per stage under `sql/` and the per-stage outputs under `stages/`, so the
  exact SQL a release emits is readable without running anything. The two
  smaller datasets carry outputs and models, no SQL.
- A **Splink parity oracle** (`er-match/tests/gen_match_golden.py`) calls Splink
  directly with the same committed model and frozen TF; the Rust path reproduces
  it to 1e-10. Which Splink produced a given baseline is recorded in that
  dataset's `MANIFEST.generated_with`, not here.

How to regenerate, what the manifest gate catches, and why the comparison runs
in SQL rather than over formatted text:
[references/goldens-and-oracles.md](references/goldens-and-oracles.md).

## Common workflows

**"These two records should have matched."** Work down the pipeline, not up.
Did a blocking rule ever emit the pair (`__dedupe__blocked_id_pairs`)? If not,
no score exists and the comparison config is irrelevant — it is a blocking
problem. If it did, read its `__dedupe__comparison_vectors` row for the gamma
levels, then `__dedupe__match_weight_parts` for which comparison contributed
what.

**"Scores changed and I did not expect them to."** Check `(model_version,
tf_snapshot_id)` first. By INV-SCORE nothing else in the run can move a score
except the records' own content hashes.

**Adding or changing a comparison level.** Change the settings template, then
retrain under a new `model_version` — editing levels under an existing model
version violates INV-SCORE. Then regenerate the affected goldens and say so in
the commit.

**Incremental diverged from full.** First assert the five INV-EQ preconditions
actually held; a violated precondition is a correct divergence, not a bug. If
they held, it is a real failure. If one was violated, `er correct` is the
repair.

**Changing clustering.** `er-reconcile` applies assertions before clustering,
so CONTRADICTION-1 is evaluated on the always-closure, not pairwise. A `never`
constraint holds across transitive connections, not just the scored pair.
