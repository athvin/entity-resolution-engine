# The `__dedupe__` SQL stages

## Contents

- Where the intermediates live, and why
- The predict backbone, stage by stage
- Blocking: the cross-rule dedup trick
- Exploding rules
- Clustering
- Training and accuracy families
- Incremental
- Reading a stage when debugging

## Where the intermediates live, and why

Every intermediate is named `__dedupe__*` and is materialized in DuckDB's
**native temp catalog**, not in the lake. The reason is a hard DuckLake
limitation: DuckLake cannot host temp tables. So the shape of every run is

- inputs read from the attached `lake` catalog by cross-catalog `SELECT`,
- all intermediates in the temp catalog,
- final outputs written back to the lake in **dedicated single-catalog
  transactions**.

That last point is why a write stage looks oddly isolated: mixing a lake write
with cross-catalog reads in one transaction is what it is avoiding.

The prefix was renamed from `__splink__df_*`. Splink survives on purpose as
provenance (the parity oracle, the `splink_udfs` extension,
`MANIFEST.generated_with`, historical banners in the spec) — those are not
leftovers to tidy.

## The predict backbone, stage by stage

| Stage | What it holds |
|---|---|
| `__dedupe__concat` | the vertical concat of all inputs — one row per record, source-tagged |
| `__dedupe__concat_with_tf` | the same, joined to the term-frequency tables; `_left` / `_right` variants exist for link-type runs |
| `__dedupe__blocked_id_pairs` | candidate pairs: one `UNION ALL` arm per blocking rule, each carrying `match_key` = the rule's index as a string literal. `_non_unique` is the pre-dedup form |
| `blocked_with_cols` | candidates widened with the comparison columns from both sides |
| `__dedupe__comparison_vectors` | the gamma level per comparison, as `CASE` expressions |
| `__dedupe__match_weight_parts` | per-comparison `mw_<name>` columns, plus `mw_tf_adj_<name>` where TF adjustment applies |
| `__dedupe__predict` | the summed match weight and the match probability |

Everything upstream of `__dedupe__predict` is deterministic given the model,
the TF tables and the inputs. That is what makes a golden fixture per stage a
usable contract.

## Blocking: the cross-rule dedup trick

Rules are evaluated in declaration order, and each arm excludes pairs an
earlier rule already produced:

```
AND NOT coalesce((previous rule's predicate), false)
```

Two things follow:

- **Rule order is semantically significant.** `match_key` records which rule
  first emitted a pair, so reordering rules rewrites `match_key` values even
  though the candidate *set* is unchanged.
- `coalesce(…, false)` is doing real work: a NULL from an earlier predicate must
  not suppress the pair, which is what a bare `NOT (predicate)` would do.

A rule's `referenced_columns()` is what the engine uses to decide which columns
must be present for it to run; `is_exploding()` marks the array case below.

## Exploding rules

A rule may block on an array column, which requires unnesting before pairing —
hence `__dedupe__concat_unnested` and the
`__dedupe__marginal_exploded_ids_blocking_rule_mk_*` family. Exploding rules
are the ones most likely to blow up candidate counts, and they have their own
golden fixture set (`rust/fixtures/golden/exploding`) for that reason.

## Clustering

| Stage | Role |
|---|---|
| `__dedupe__edges_from_predict` | scored pairs above threshold become graph edges |
| `__dedupe__all_nodes` | every record in scope, including singletons |
| `__dedupe__neighbours`, `__dedupe__filtered_neighbours_*` | adjacency |
| `__dedupe__representatives{,_stable_,_unstable_}` | min-representative label propagation; converged clusters are marked `stable` and retired from further iteration |
| `__dedupe__clustering_output_final`, `__dedupe__clustered_with_input_data` | the result, and the result joined back to inputs |
| `__dedupe__counts_per_cluster`, `__dedupe__graph_metrics_*` | cluster size and graph diagnostics |

The `stable` flag is the performance mechanism: without it every iteration
rescans every cluster. When debugging a clustering change, check whether a
cluster was retired early rather than assuming the propagation is wrong.

## Training and accuracy families

- `__dedupe__m_u_counts` — the EM counting stage.
- `__dedupe__labels_*` — a long chain for label-driven work: `labels_prepared`,
  `labels_with_predictions`, `labels_with_pos_neg*`, the `_grouped_with_stats`
  variants, and the truth-stat adjustments.
- `__dedupe__truth_space_table`, `__dedupe__agreement_pattern_counts`,
  `__dedupe__multi_threshold_summary` — accuracy reporting.
- `__dedupe__unlinkables{,_proportions}`, `__dedupe__round_self_link` — the
  self-link diagnostics.
- `__dedupe__all_column_value_frequencies`, `__dedupe__completeness`,
  `__dedupe__concat_sample` — profiling and u-estimation support.
- `__dedupe__deterministic_blocked` — the deterministic-rule pass used for the
  prior.

## Incremental

`__dedupe__incremental_seed_nodes`, `__dedupe__incremental_seed_edges`,
`__dedupe__incremental_reps`. The seeds are the affected subgraph: new or
changed records, expanded to the whole existing entities they touch plus the
relevant current edges. `predict_between` and `predict_within` in
`incremental.rs` are the two scoring shapes (new-vs-corpus, and new-vs-new).

The reason an incremental pass cannot reproduce a full pass unaided is visible
right here: the seeds never contain a pair of two records that were both
already in the corpus. That is the first INV-EQ loss vector.

## Reading a stage when debugging

Work **down** the pipeline, not up. The first question for "why didn't these
match?" is always whether a candidate pair exists at all:

1. `__dedupe__blocked_id_pairs` — is the pair there? If not, this is a blocking
   problem and nothing about comparisons or weights is relevant yet.
2. `__dedupe__comparison_vectors` — what gamma level did each comparison land
   on? A surprising level usually means the standardized value is not what you
   expect, which is an `er-standardize` question.
3. `__dedupe__match_weight_parts` — which comparison contributed what, and did
   a `mw_tf_adj_` column move it.
4. `__dedupe__predict` — the total and the probability.

The committed golden SQL files are named for these stages
(`predict_01___dedupe__blocked_id_pairs.sql` and so on), so the exact SQL a
given release emits is readable without running anything.
