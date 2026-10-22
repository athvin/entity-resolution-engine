---
name: er-matching
description: Explains this repo's Fellegi–Sunter matching engine — blocking rules, comparison levels, m/u probabilities, match weights, EM training, term-frequency adjustment, clustering and incremental scoring — as implemented in dedupe-core, er-match, er-compile and er-reconcile, together with the named invariants (INV-SCORE, INV-EQ, INV-PERM, CONTRADICTION-1) that scores and entity ids must satisfy. Use when changing blocking or comparison config, training or retraining a model, debugging why two records did or did not match, working on term frequency or incremental scoring, touching the `__dedupe__` SQL stages, or when a golden fixture or a `match_scores` expectation fails. For running the pipeline or the `er` CLI see er-engine-ops; for declaring a table or a SQL statement see entity-framework.
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
| The ER stage | `er-match` | `int_std_records` + a committed model → `match_scores`; the **frozen-TF** path |
| Tenant config → model | `er-compile` | a tenant's declared `comparisons:`/`blocking:` → the settings document |
| Entity formation | `er-reconcile` | assertion-adjusted clustering, entity identity, never-cut |

## The one trap that silently corrupts scores

`dedupe-core` exposes two prediction entry points and they are **not**
interchangeable:

- `run_predict` computes term frequency **from the input it is given**.
- `run_predict_with_tf` scores against **TF tables you supply**.

The ER pipeline must always take the second path, with TF built from a
`tf_lookup` snapshot. That is **D4/INV-SCORE**:

> `match_probability` is a pure function of `(model_version, tf_snapshot_id,
> rec_a_key, rec_b_key, rec_a_content_hash, rec_b_content_hash)`.

Nothing in that tuple is "the batch". If TF were recomputed per run, the same
pair would score differently depending on which other records happened to be
present — so an incremental batch could never agree with a full run, and
yesterday's score would not reproduce. `er-match` builds the TF tables from the
snapshot and calls `run_predict_with_tf` for exactly this reason.

**Reviewing a change to the scoring path, check this first.** A switch to
`run_predict` compiles, passes a unit test on one batch, and breaks
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

```
__dedupe__concat            vertical concat of inputs
  → __dedupe__concat_with_tf        TF join (_left/_right variants)
  → __dedupe__blocked_id_pairs      one UNION ALL arm per blocking rule
  → blocked_with_cols
  → __dedupe__comparison_vectors    the gamma CASE columns
  → __dedupe__match_weight_parts    per-comparison mw_ / mw_tf_adj_ columns
  → __dedupe__predict               summed weight + match probability
```

Stage-by-stage detail, the clustering and training stage families, and the
cross-rule dedup idiom: [references/pipeline-stages.md](references/pipeline-stages.md).

`__dedupe__*` was renamed from `__splink__df_*`. Splink nonetheless survives
**deliberately** as provenance — the parity oracle, the `splink_udfs`
extension, `MANIFEST.generated_with`, and historical banners in the spec. Do
not "clean it up".

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
full agreement is scaled to **60 bits**, and *louder is not safer*. On
`base_10`, 20 bits finds 18 clusters (it fails to merge) and 120 bits finds 9 —
it has started **merging distinct people**. 55–60 reproduces the committed
membership exactly. Treat the scale as calibrated, not as a tuning knob:
[references/config-to-model.md](references/config-to-model.md).

## The goldens are the contract

Changes to the engine are governed by captured fixtures, not by the SQL
analyser — `dedupe-core`'s model SQL is deliberately outside `er-sql-check`'s
remit and under an execution oracle instead.

- Golden SQL + output fixtures per stage, gated by a SHA-256 `MANIFEST.json`
  so a partial regeneration cannot pass unnoticed.
- A **Splink 5.0.0 parity oracle** (`er-match/tests/gen_match_golden.py`) calls
  Splink directly with the same committed model and frozen TF; the Rust path
  reproduces it to 1e-10.

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
