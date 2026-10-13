//! Assembles the predict-pipeline stages (single-chunk path): blocked pairs
//! are materialized from
//! `__splink__df_concat`; comparison vectors (and later scoring) run as one
//! CTE pipeline over the blocked pairs with `__splink__df_concat_with_tf`
//! re-derived as a CTE.

use crate::error::Result;
use crate::session::{Session, TableHandle};
use crate::settings::Settings;
use crate::sql::pipeline::CtePipeline;
use crate::stages::block::{self, BLOCKED_ID_PAIRS};
use crate::stages::concat::{self, DF_CONCAT};
use crate::stages::score;
use crate::stages::tf::{self, DF_CONCAT_WITH_TF};
use crate::stages::uid_expr_for;
use crate::stages::vectors::{self, BLOCKED_WITH_COLS, DF_COMPARISON_VECTORS};

/// Materialize `__splink__df_concat` from a single registered input table.
pub fn materialize_concat(session: &Session, input: &TableHandle) -> Result<TableHandle> {
    let mut p = CtePipeline::new().with_input(input);
    p.enqueue(concat::concat_sql_single(input), DF_CONCAT);
    session.materialize(&p)
}

/// Materialize `__splink__df_concat` from multiple registered inputs (link
/// types), prepending each dataset's display name as the `source_dataset`
/// literal. Column order comes from the first input.
pub fn materialize_concat_multi(
    session: &Session,
    inputs: &[(&TableHandle, &str)],
    settings: &Settings,
) -> Result<TableHandle> {
    if inputs.len() == 1 && !settings.source_dataset_required() {
        return materialize_concat(session, inputs[0].0);
    }
    // Inputs are referenced by physical name inside the union SQL, so they are
    // NOT registered as pipeline inputs (that would rewrite the source_dataset
    // string literals).
    let mut p = CtePipeline::new();
    p.enqueue(
        concat::concat_sql_union(inputs, &settings.source_dataset_column),
        DF_CONCAT,
    );
    session.materialize(&p)
}

/// Compute and materialize one TF table per TF column (the cached artifacts).
pub fn materialize_tf_tables(
    session: &Session,
    settings: &Settings,
    concat: &TableHandle,
) -> Result<Vec<TableHandle>> {
    settings
        .tf_columns()
        .iter()
        .map(|col| {
            let mut p = CtePipeline::new().with_input(concat);
            p.enqueue(
                tf::tf_table_sql(col, &settings.tf_prefix),
                tf::tf_table_name(col),
            );
            session.materialize(&p)
        })
        .collect()
}

/// Materialize `__splink__blocked_id_pairs` (blocking reads the concat, not
/// concat_with_tf). Materialized rather than inlined, sidestepping the
/// DuckDB UNION-ALL pushdown regression.
pub fn materialize_blocked_pairs(
    session: &Session,
    settings: &Settings,
    concat: &TableHandle,
) -> Result<TableHandle> {
    materialize_blocked_pairs_chunk(session, settings, concat, None)
}

/// As [`materialize_blocked_pairs`], but restricted to one hash chunk of the
/// right-hand records (`right_chunk = Some((i, n))` keeps pairs whose right
/// record satisfies `hash(uid_r) % n == i`). `None` (or `n == 1`) blocks every
/// pair, byte-identical to the unchunked path. Used by [`run_predict_chunked`]
/// to process a large candidate set in bounded-memory passes.
pub fn materialize_blocked_pairs_chunk(
    session: &Session,
    settings: &Settings,
    concat: &TableHandle,
    right_chunk: Option<(u32, u32)>,
) -> Result<TableHandle> {
    let uid = uid_expr_for(settings);
    let rules = &settings.blocking_rules;

    if !rules
        .iter()
        .any(super::blocking::BlockingRule::is_exploding)
    {
        // Fast path: no array explosion, a single UNION-ALL of scalar arms.
        let mut p = CtePipeline::new().with_input(concat);
        p.enqueue(
            block::blocked_id_pairs_sql(
                rules,
                &uid,
                settings.link_type,
                &settings.source_dataset_column,
                DF_CONCAT,
                &[],
                right_chunk,
            ),
            BLOCKED_ID_PAIRS,
        );
        return session.materialize(&p);
    }

    materialize_blocked_pairs_exploding(session, settings, concat, &uid, right_chunk)
}

/// Exploding-rule path: materialize each exploding rule's marginal id-pairs
/// table (UNNEST the concat, self-join, apply the dedupe where-clause and the
/// "AND NOT preceding" exclusion), then UNION ALL all arms into
/// `__splink__blocked_id_pairs_non_unique` and collapse to unique pairs by
/// `min(match_key)`. `right_chunk` filters both the scalar arms
/// and the marginal self-joins on the right record's hash; since a pair's right
/// record is fixed (`uid_l < uid_r`), both arms land in the same chunk, so the
/// per-chunk `min(match_key)` dedup stays correct.
fn materialize_blocked_pairs_exploding(
    session: &Session,
    settings: &Settings,
    concat: &TableHandle,
    uid: &crate::stages::UidExpr,
    right_chunk: Option<(u32, u32)>,
) -> Result<TableHandle> {
    let rules = &settings.blocking_rules;
    let sd_col = if settings.source_dataset_required() {
        Some(settings.source_dataset_column.as_str())
    } else {
        None
    };

    // One materialized marginal table per exploding rule.
    let mut marginals: Vec<(usize, TableHandle)> = Vec::new();
    for (i, rule) in rules.iter().enumerate() {
        if !rule.is_exploding() {
            continue;
        }
        let preceding: Vec<&_> = rules[..i].iter().collect();
        let retain =
            block::blocking_retain_columns(&preceding, rule, &settings.unique_id_column, sd_col)?;
        let unnest_sql = block::explode_arrays_sql(DF_CONCAT, &rule.arrays_to_explode, &retain);
        let marginal_sql = block::marginal_exploded_id_pairs_sql(
            rule,
            &preceding,
            uid,
            settings.link_type,
            &settings.source_dataset_column,
            block::DF_CONCAT_UNNESTED,
            right_chunk,
        );
        let mut mp = CtePipeline::new().with_input(concat);
        mp.enqueue(unnest_sql, block::DF_CONCAT_UNNESTED);
        mp.enqueue(
            marginal_sql,
            block::marginal_exploded_table_name(rule.index),
        );
        marginals.push((rule.index, session.materialize(&mp)?));
    }

    let marginal_refs: Vec<(usize, String)> = marginals
        .iter()
        .map(|(idx, h)| (*idx, h.physical.clone()))
        .collect();

    // UNION ALL (scalar arms + exploding arms reading their marginals) ->
    // non-unique, then `min(match_key)` dedup.
    let mut p = CtePipeline::new().with_input(concat);
    p.enqueue(
        block::blocked_id_pairs_sql(
            rules,
            uid,
            settings.link_type,
            &settings.source_dataset_column,
            DF_CONCAT,
            &marginal_refs,
            right_chunk,
        ),
        block::BLOCKED_ID_PAIRS_NON_UNIQUE,
    );
    p.enqueue(
        block::dedup_non_unique_sql(block::BLOCKED_ID_PAIRS_NON_UNIQUE),
        BLOCKED_ID_PAIRS,
    );
    let out = session.materialize(&p)?;

    for (_, h) in &marginals {
        session.drop_table(h)?;
    }
    Ok(out)
}

/// Pipeline from blocked id pairs through `__splink__df_comparison_vectors`.
pub fn comparison_vectors_pipeline(
    settings: &Settings,
    concat: &TableHandle,
    tf_tables: &[TableHandle],
    blocked_pairs: &TableHandle,
) -> CtePipeline {
    let uid = uid_expr_for(settings);
    let mut p = CtePipeline::new()
        .with_input(concat)
        .with_input(blocked_pairs);
    for t in tf_tables {
        p.add_input(t);
    }
    p.enqueue(
        tf::concat_with_tf_sql(&settings.tf_columns(), &settings.tf_prefix),
        DF_CONCAT_WITH_TF,
    );
    p.enqueue(
        vectors::blocked_with_cols_sql(settings, &uid),
        BLOCKED_WITH_COLS,
    );
    p.enqueue(
        vectors::comparison_vectors_sql(settings),
        DF_COMPARISON_VECTORS,
    );
    p
}

/// Deterministic link: candidate pairs from the blocking rules joined back to
/// their record columns, with NO probabilistic scoring. The output is the
/// `blocked_with_cols` table (uid + comparison/tf columns as `_l`/`_r`, plus
/// `match_key`). Use for strict rule-based linkage.
pub fn deterministic_link(
    session: &Session,
    settings: &Settings,
    concat: &TableHandle,
    tf_tables: &[TableHandle],
) -> Result<TableHandle> {
    let uid = uid_expr_for(settings);
    let blocked = materialize_blocked_pairs(session, settings, concat)?;
    let mut p = CtePipeline::new().with_input(concat).with_input(&blocked);
    for t in tf_tables {
        p.add_input(t);
    }
    p.enqueue(
        tf::concat_with_tf_sql(&settings.tf_columns(), &settings.tf_prefix),
        DF_CONCAT_WITH_TF,
    );
    p.enqueue(
        vectors::blocked_with_cols_sql(settings, &uid),
        BLOCKED_WITH_COLS,
    );
    let out = session.materialize(&p)?;
    session.drop_table(&blocked)?;
    Ok(out)
}

/// Full predict pipeline: comparison vectors -> match weight parts ->
/// `__splink__df_predict` scored with the settings' current parameters.
pub fn predict_pipeline(
    settings: &Settings,
    concat: &TableHandle,
    tf_tables: &[TableHandle],
    blocked_pairs: &TableHandle,
    threshold_match_weight: Option<f64>,
) -> Result<CtePipeline> {
    let mut p = comparison_vectors_pipeline(settings, concat, tf_tables, blocked_pairs);
    p.enqueue(
        score::match_weight_parts_sql(settings)?,
        score::DF_MATCH_WEIGHT_PARTS,
    );
    p.enqueue(
        score::predict_sql(settings, settings.prior, false, threshold_match_weight)?,
        score::DF_PREDICT,
    );
    Ok(p)
}

/// Convenience: run the whole predict flow from a registered input table.
pub fn run_predict(
    session: &Session,
    settings: &Settings,
    input: &TableHandle,
    threshold_match_weight: Option<f64>,
) -> Result<TableHandle> {
    let concat = materialize_concat(session, input)?;
    let tf_tables = materialize_tf_tables(session, settings, &concat)?;
    let blocked = materialize_blocked_pairs(session, settings, &concat)?;
    let pipeline = predict_pipeline(
        settings,
        &concat,
        &tf_tables,
        &blocked,
        threshold_match_weight,
    )?;
    session.materialize(&pipeline)
}

/// Run predict in `num_chunks` bounded-memory passes. Pass `i` scores only the
/// candidate pairs whose *right* record hashes to chunk `i` (`hash(uid_r) % n ==
/// i`); the passes are unioned into one result. The output is identical to
/// [`run_predict`] — each pair belongs to exactly one chunk — so this is a pure
/// memory/scaling lever, not a semantic change (`num_chunks <= 1` dispatches
/// straight to `run_predict`). Chunking bounds the peak working set: the blocked
/// pairs and scoring intermediates for one pass are ~`1/num_chunks` of the whole,
/// so a candidate set too large to score in a single shot still completes. The
/// concat and TF tables are built once and shared across passes. See
/// PERFORMANCE.md.
pub fn run_predict_chunked(
    session: &Session,
    settings: &Settings,
    input: &TableHandle,
    threshold_match_weight: Option<f64>,
    num_chunks: u32,
) -> Result<TableHandle> {
    if num_chunks <= 1 {
        return run_predict(session, settings, input, threshold_match_weight);
    }
    let concat = materialize_concat(session, input)?;
    let tf_tables = materialize_tf_tables(session, settings, &concat)?;

    // First pass seeds the accumulator table; later passes INSERT into it and
    // drop their own per-pass output, so only one pass's intermediates plus the
    // growing result are live at a time.
    let mut acc: Option<TableHandle> = None;
    for i in 0..num_chunks {
        let blocked =
            materialize_blocked_pairs_chunk(session, settings, &concat, Some((i, num_chunks)))?;
        let pipeline = predict_pipeline(
            settings,
            &concat,
            &tf_tables,
            &blocked,
            threshold_match_weight,
        )?;
        let part = session.materialize(&pipeline)?;
        session.drop_table(&blocked)?;
        match &acc {
            None => acc = Some(part),
            Some(a) => {
                session.execute_batch(&format!(
                    "INSERT INTO {} SELECT * FROM {}",
                    a.physical, part.physical
                ))?;
                session.drop_table(&part)?;
            }
        }
    }
    // num_chunks >= 2 runs at least one pass, so acc is always Some here.
    acc.ok_or_else(|| crate::error::DedupeError::Other("predict produced no chunks".to_string()))
}
