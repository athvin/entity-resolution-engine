//! Accuracy evaluation from ground-truth labels (the label-column path).
//!
//! Produces the "truth-space table": one row per match-weight threshold with
//! the confusion-matrix counts (TP/FP/TN/FN) and derived metrics (precision,
//! recall, F-scores, specificity, NPV, accuracy, φ, ROC rates). This is the
//! data behind accuracy / ROC / precision-recall charts; no charts
//! are produced here.
//!
//! The label-column path assigns truth from equality of a ground-truth column
//! (e.g. a known cluster id): candidate pairs are everything the model's
//! blocking rules generate, UNION same-label pairs the blocking missed (via an
//! appended label-equality blocking rule). Pairs never materialised are
//! implicit true negatives (the "ghost" adjustment against the full cartesian
//! count).

use crate::blocking::BlockingRule;
use crate::error::Result;
use crate::predict;
use crate::session::{Session, TableHandle};
use crate::settings::Settings;
use crate::sql::literals::pyrepr;
use crate::sql::pipeline::CtePipeline;
use crate::stages::block::BLOCKED_ID_PAIRS;
use crate::stages::score::{self, DF_MATCH_WEIGHT_PARTS, DF_PREDICT};
use crate::stages::uid_expr_for;
use crate::training::prior::total_possible_comparisons;

/// Rewrite a blocking rule from table-prefixed form (`l."col"`, `r."col"`) to
/// the column-suffix form (`"col_l"`, `"col_r"`) used on prediction output.
fn br_to_suffix_form(rule: &str) -> String {
    use regex::Regex;
    let mut s = rule.to_string();
    for (pat, rep) in [
        (r#"\bl\."([^"]+)""#, "\"${1}_l\""),
        (r#"\br\."([^"]+)""#, "\"${1}_r\""),
        (r"\bl\.([A-Za-z_][A-Za-z0-9_]*)", "${1}_l"),
        (r"\br\.([A-Za-z_][A-Za-z0-9_]*)", "${1}_r"),
    ] {
        s = Regex::new(pat).unwrap().replace_all(&s, rep).into_owned();
    }
    s
}

/// `found_by_blocking_rules` expression (labels-table path): OR of the model's
/// blocking rules in suffix form, each coalesced to false; `1=1` if none.
fn found_by_blocking_rules_expr(settings: &Settings) -> String {
    if settings.blocking_rules.is_empty() {
        return "1=1".to_string();
    }
    settings
        .blocking_rules
        .iter()
        .map(|br| format!("(coalesce({}, false))", br_to_suffix_form(&br.sql)))
        .collect::<Vec<_>>()
        .join(" OR ")
}

pub struct AccuracyOptions {
    pub label_column: String,
    /// A clerical/label score at or above this counts as a true match.
    pub threshold_actual: f64,
    /// Round match weights to this granularity before enumerating thresholds
    /// (`None` = every distinct weight); 0.1 is the typical choice.
    pub match_weight_round_to_nearest: Option<f64>,
    /// Push clerical positives missed by blocking to a sentinel threshold so
    /// they count as false negatives at every real threshold.
    pub positives_not_captured_by_blocking_rules_scored_as_zero: bool,
}

impl Default for AccuracyOptions {
    fn default() -> Self {
        AccuracyOptions {
            label_column: "cluster".to_string(),
            threshold_actual: 0.5,
            match_weight_round_to_nearest: Some(0.1),
            positives_not_captured_by_blocking_rules_scored_as_zero: true,
        }
    }
}

/// Build the labelled, scored pair set: the model's blocking rules plus an
/// appended label-equality rule, scored, with `clerical_match_score` (binary
/// label equality) and `found_by_blocking_rules` (produced by a real model
/// rule rather than the appended label rule).
fn labelled_predictions(
    session: &Session,
    settings: &Settings,
    concat: &TableHandle,
    tf_tables: &[TableHandle],
    label_column: &str,
) -> Result<TableHandle> {
    let uid = uid_expr_for(settings);
    let n_model_rules = settings.blocking_rules.len();

    // Augmented rules: model rules (indices 0..n) + label-equality rule (n).
    let mut rules = settings.blocking_rules.clone();
    rules.push(BlockingRule {
        index: n_model_rules,
        sql: format!("l.\"{label_column}\" = r.\"{label_column}\""),
        arrays_to_explode: Vec::new(),
    });

    let mut bp = CtePipeline::new().with_input(concat);
    bp.enqueue(
        crate::stages::block::blocked_id_pairs_sql(
            &rules,
            &uid,
            settings.link_type,
            &settings.source_dataset_column,
            concat.templated.as_str(),
            &[],
            None,
        ),
        crate::stages::block::BLOCKED_ID_PAIRS,
    );
    let blocked = session.materialize(&bp)?;

    // Retain the label column through scoring so clerical truth can be derived.
    let mut acc_settings = settings.clone();
    if !acc_settings
        .additional_columns_to_retain
        .iter()
        .any(|c| c == label_column)
    {
        acc_settings
            .additional_columns_to_retain
            .push(label_column.to_string());
    }

    // comparison vectors -> match weight parts -> predict (no threshold).
    let mut p = predict::comparison_vectors_pipeline(&acc_settings, concat, tf_tables, &blocked);
    p.enqueue(
        score::match_weight_parts_sql(&acc_settings)?,
        DF_MATCH_WEIGHT_PARTS,
    );
    p.enqueue(
        score::predict_sql(&acc_settings, acc_settings.prior, false, None)?,
        DF_PREDICT,
    );
    let predictions = session.materialize(&p)?;
    session.drop_table(&blocked)?;

    // labels_with_predictions: add clerical_match_score + found_by_blocking_rules.
    let mut lp = CtePipeline::new().with_input(&predictions);
    lp.enqueue(
        format!(
            "select *,\n  case when \"{label_column}_l\" = \"{label_column}_r\" then cast(1.0 as float8) else cast(0.0 as float8) end as clerical_match_score,\n  not (cast(match_key as int) = {n_model_rules}) as found_by_blocking_rules\nfrom {}",
            predictions.templated
        ),
        "__splink__labels_with_predictions",
    );
    let out = session.materialize(&lp)?;
    session.drop_table(&predictions)?;
    Ok(out)
}

/// The 7-step truth-space SQL over a `__splink__labels_with_predictions`-shaped
/// table (needs `match_weight`, `clerical_match_score`, `found_by_blocking_rules`).
fn truth_space_sql(
    labels_with_predictions: &str,
    threshold_actual: f64,
    round_to_nearest: Option<f64>,
    total_labels: Option<f64>,
    positives_as_zero: bool,
) -> String {
    let truth_thres_expr = match round_to_nearest {
        Some(r) => format!(
            "cast({r} as float) * (round(match_weight/{r}))",
            r = pyrepr(r)
        ),
        None => "match_weight".to_string(),
    };
    let ta = pyrepr(threshold_actual);
    let tt_adj = if positives_as_zero {
        "CASE WHEN found_by_blocking_rules then truth_threshold ELSE cast(-999 as float8) END"
    } else {
        "truth_threshold"
    };
    // The labels-column path adds the "ghost" true-negatives (all cartesian
    // pairs not materialised); the labels-table path (total_labels = None)
    // does not adjust.
    let (adj_cumulative_neg, adj_total_neg, adj_total_labels, adj_scored_below) = match total_labels
    {
        Some(tl) => {
            let s = format!("cast({} as float8)", pyrepr(tl));
            let extra = format!("({s} - total_clerical_positives - total_clerical_negatives)");
            (
                format!("cumulative_clerical_negatives_below_threshold + {extra}"),
                format!("{s} - total_clerical_positives"),
                s,
                format!("num_labels_scored_below_threshold + {extra}"),
            )
        }
        None => (
            "cumulative_clerical_negatives_below_threshold".to_string(),
            "total_clerical_negatives".to_string(),
            "total_clerical_labels".to_string(),
            "num_labels_scored_below_threshold".to_string(),
        ),
    };

    format!(
        r"WITH
__splink__labels_with_pos_neg as (
    select *,
        {truth_thres_expr} as truth_threshold,
        case when clerical_match_score >= {ta} then 1 else 0 end as clerical_positive,
        case when clerical_match_score >= {ta} then 0 else 1 end as clerical_negative
    from {labels_with_predictions}
),
__splink__labels_with_pos_neg_tt_adj as (
    select *, {tt_adj} as truth_threshold_adj
    from __splink__labels_with_pos_neg
),
__splink__labels_with_pos_neg_grouped as (
    select
        truth_threshold_adj as truth_threshold,
        count(*) as num_records_in_row,
        sum(clerical_positive) as clerical_positive,
        sum(clerical_negative) as clerical_negative
    from __splink__labels_with_pos_neg_tt_adj
    group by truth_threshold_adj
),
__splink__labels_with_pos_neg_grouped_with_stats as (
    select
        truth_threshold,
        (sum(clerical_positive) over (order by truth_threshold desc))
            as cumulative_clerical_positives_at_or_above_threshold,
        (sum(clerical_negative) over (order by truth_threshold)) - clerical_negative
            as cumulative_clerical_negatives_below_threshold,
        (select sum(clerical_positive) from __splink__labels_with_pos_neg_grouped) as total_clerical_positives,
        (select sum(clerical_negative) from __splink__labels_with_pos_neg_grouped) as total_clerical_negatives,
        (select sum(num_records_in_row) from __splink__labels_with_pos_neg_grouped) as total_clerical_labels,
        -num_records_in_row + sum(num_records_in_row) over (order by truth_threshold)
            as num_labels_scored_below_threshold,
        sum(num_records_in_row) over (order by truth_threshold desc)
            as num_labels_scored_at_or_above_threshold
    from __splink__labels_with_pos_neg_grouped
),
__splink__labels_with_pos_neg_grouped_with_stats_adj as (
    select
        truth_threshold,
        cumulative_clerical_positives_at_or_above_threshold,
        {adj_cumulative_neg}
            as cumulative_clerical_negatives_below_threshold,
        total_clerical_positives,
        {adj_total_neg} as total_clerical_negatives,
        {adj_total_labels} as total_clerical_labels,
        {adj_scored_below} as num_labels_scored_below_threshold,
        num_labels_scored_at_or_above_threshold
    from __splink__labels_with_pos_neg_grouped_with_stats
),
__splink__labels_with_pos_neg_grouped_with_truth_stats as (
    select
        truth_threshold,
        total_clerical_labels,
        total_clerical_positives as P,
        total_clerical_negatives as N,
        num_labels_scored_at_or_above_threshold - cumulative_clerical_positives_at_or_above_threshold as FP,
        cumulative_clerical_positives_at_or_above_threshold as TP,
        num_labels_scored_below_threshold - cumulative_clerical_negatives_below_threshold as FN,
        cumulative_clerical_negatives_below_threshold as TN
    from __splink__labels_with_pos_neg_grouped_with_stats_adj
)
select
    truth_threshold,
    power(2, truth_threshold) / (1 + power(2, truth_threshold)) as match_probability,
    cast(total_clerical_labels as float8) as total_clerical_labels,
    cast(P as float8) as p,
    cast(N as float8) as n,
    cast(TP as float8) as tp,
    cast(TN as float8) as tn,
    cast(FP as float8) as fp,
    cast(FN as float8) as fn,
    cast(P/total_clerical_labels as float8) as P_rate,
    cast(N as float)/total_clerical_labels as N_rate,
    cast(TP as float8)/P as tp_rate,
    cast(TN as float8)/N as tn_rate,
    cast(FP as float8)/N as fp_rate,
    cast(FN as float8)/P as fn_rate,
    case when TP+FP=0 then 1 else cast(TP as float8)/(TP+FP) end as precision,
    cast(TP as float8)/P as recall,
    cast(TN as float8)/N as specificity,
    case when TN+FN=0 then 1 else cast(TN as float8)/(TN+FN) end as npv,
    cast(TP+TN as float8)/(P+N) as accuracy,
    cast(2.0*TP/(2*TP + FN + FP) as float8) as f1,
    cast(5.0*TP/(5*TP + 4*FN + FP) as float8) as f2,
    cast(1.25*TP/(1.25*TP + 0.25*FN + FP) as float8) as f0_5,
    cast(4.0*TP*TN/((4.0*TP*TN) + ((TP + TN)*(FP + FN))) as float8) as p4,
    case when TN+FN=0 or TP+FP=0 or P=0 or N=0 then 0
        else cast((TP*TN)-(FP*FN) as float8)/sqrt((TP+FP)*P*N*(TN+FN)) end as phi
from __splink__labels_with_pos_neg_grouped_with_truth_stats
where truth_threshold >= cast(-998 as float8)
order by truth_threshold"
    )
}

/// Compute the truth-space table from a ground-truth label column.
pub fn truth_space_from_labels_column(
    session: &Session,
    settings: &Settings,
    concat: &TableHandle,
    tf_tables: &[TableHandle],
    opts: &AccuracyOptions,
) -> Result<TableHandle> {
    let total_labels = total_possible_comparisons(session, concat, settings)?;
    let labelled = labelled_predictions(session, settings, concat, tf_tables, &opts.label_column)?;

    let mut p = CtePipeline::new().with_input(&labelled);
    p.enqueue(
        truth_space_sql(
            &labelled.templated,
            opts.threshold_actual,
            opts.match_weight_round_to_nearest,
            Some(total_labels),
            opts.positives_not_captured_by_blocking_rules_scored_as_zero,
        ),
        "__splink__truth_space_table",
    );
    let out = session.materialize(&p)?;
    session.drop_table(&labelled)?;
    Ok(out)
}

/// Score a labels table's pairs:
/// normalize each labelled pair to lower-id-on-left, score it with the model,
/// and attach `clerical_match_score` (verbatim) and `found_by_blocking_rules`
/// (OR of model rules). Returns a `labels_with_predictions`-shaped table.
/// `labels` must have `<uid>_l`, `<uid>_r`, `clerical_match_score` (and
/// `<source_dataset>_l/_r` for link types).
pub fn labels_table_predictions(
    session: &Session,
    settings: &Settings,
    concat: &TableHandle,
    tf_tables: &[TableHandle],
    labels: &TableHandle,
) -> Result<TableHandle> {
    let uid = uid_expr_for(settings);
    let uid_col = &settings.unique_id_column;
    let id_l = uid.render_suffixed("l");
    let id_r = uid.render_suffixed("r");

    // Lower-id-on-left normalization of the labelled pairs.
    let mut swap_cols = Vec::new();
    if settings.source_dataset_required() {
        let sd = &settings.source_dataset_column;
        swap_cols.push(format!(
            "case when {id_l} < {id_r} then \"{sd}_l\" else \"{sd}_r\" end as \"{sd}_l\""
        ));
        swap_cols.push(format!(
            "case when {id_l} < {id_r} then \"{sd}_r\" else \"{sd}_l\" end as \"{sd}_r\""
        ));
    }
    swap_cols.push(format!(
        "case when {id_l} < {id_r} then \"{uid_col}_l\" else \"{uid_col}_r\" end as \"{uid_col}_l\""
    ));
    swap_cols.push(format!(
        "case when {id_l} < {id_r} then \"{uid_col}_r\" else \"{uid_col}_l\" end as \"{uid_col}_r\""
    ));
    let mut lp = CtePipeline::new().with_input(labels);
    lp.enqueue(
        format!(
            "select {}, clerical_match_score from {}",
            swap_cols.join(", "),
            labels.templated
        ),
        "__splink__labels_prepared",
    );
    let prepared = session.materialize(&lp)?;

    // Turn the prepared labels into a blocked-pairs table and score via the
    // standard predict pipeline.
    let mut bp = CtePipeline::new().with_input(&prepared);
    bp.enqueue(
        format!(
            "select '0' as match_key, {id_l} as join_key_l, {id_r} as join_key_r from {}",
            prepared.templated
        ),
        BLOCKED_ID_PAIRS,
    );
    let blocked = session.materialize(&bp)?;
    let pred_pipeline = predict::predict_pipeline(settings, concat, tf_tables, &blocked, None)?;
    let predictions = session.materialize(&pred_pipeline)?;

    // Join clerical score back and compute found_by_blocking_rules.
    let fbr = found_by_blocking_rules_expr(settings);
    let join_on = if settings.source_dataset_required() {
        let sd = &settings.source_dataset_column;
        format!(
            "p.\"{sd}_l\" = lp.\"{sd}_l\" and p.\"{uid_col}_l\" = lp.\"{uid_col}_l\" and p.\"{sd}_r\" = lp.\"{sd}_r\" and p.\"{uid_col}_r\" = lp.\"{uid_col}_r\""
        )
    } else {
        format!("p.\"{uid_col}_l\" = lp.\"{uid_col}_l\" and p.\"{uid_col}_r\" = lp.\"{uid_col}_r\"")
    };
    let mut fp = CtePipeline::new()
        .with_input(&predictions)
        .with_input(&prepared);
    fp.enqueue(
        format!(
            "select p.*, lp.clerical_match_score, ({fbr}) as found_by_blocking_rules\nfrom {p} as p\ninner join {lp} as lp on {join_on}",
            p = predictions.templated,
            lp = prepared.templated,
        ),
        "__splink__labels_with_predictions",
    );
    let out = session.materialize(&fp)?;
    session.drop_table(&prepared)?;
    session.drop_table(&blocked)?;
    session.drop_table(&predictions)?;
    Ok(out)
}

/// Truth-space table from a labels table (no ghost-negative adjustment; P/N
/// are the labelled positives/negatives only).
pub fn truth_space_from_labels_table(
    session: &Session,
    settings: &Settings,
    concat: &TableHandle,
    tf_tables: &[TableHandle],
    labels: &TableHandle,
    threshold_actual: f64,
    match_weight_round_to_nearest: Option<f64>,
) -> Result<TableHandle> {
    let lwp = labels_table_predictions(session, settings, concat, tf_tables, labels)?;
    let mut p = CtePipeline::new().with_input(&lwp);
    p.enqueue(
        truth_space_sql(
            &lwp.templated,
            threshold_actual,
            match_weight_round_to_nearest,
            None,
            true,
        ),
        "__splink__truth_space_table",
    );
    let out = session.materialize(&p)?;
    session.drop_table(&lwp)?;
    Ok(out)
}

/// False positives / false negatives vs a labels table: adds a
/// `truth_status` column.
#[allow(clippy::too_many_arguments)]
pub fn prediction_errors_from_labels_table(
    session: &Session,
    settings: &Settings,
    concat: &TableHandle,
    tf_tables: &[TableHandle],
    labels: &TableHandle,
    threshold: f64,
    include_false_positives: bool,
    include_false_negatives: bool,
) -> Result<TableHandle> {
    let lwp = labels_table_predictions(session, settings, concat, tf_tables, labels)?;
    let t = pyrepr(threshold);
    let fp = format!("(clerical_match_score < {t} and match_probability > {t})");
    let fnn = format!("(clerical_match_score > {t} and match_probability < {t})");
    let mut conds = Vec::new();
    if include_false_positives {
        conds.push(fp.clone());
    }
    if include_false_negatives {
        conds.push(fnn.clone());
    }
    let where_clause = conds.join(" or ");
    let mut p = CtePipeline::new().with_input(&lwp);
    p.enqueue(
        format!(
            "select *, case when {fp} then 'FP' when {fnn} then 'FN' end as truth_status\nfrom {} where {where_clause}",
            lwp.templated
        ),
        "__splink__labels_with_fp_fn_status",
    );
    let out = session.materialize(&p)?;
    session.drop_table(&lwp)?;
    Ok(out)
}

/// Unlinkables data: score each record against itself, then the cumulative
/// proportion of records whose self-match probability falls below each level
/// ("unlinkable to themselves"). Columns: match_weight, match_probability,
/// prop, cum_prop.
pub fn unlinkables_data(
    session: &Session,
    settings: &Settings,
    concat: &TableHandle,
    tf_tables: &[TableHandle],
) -> Result<TableHandle> {
    let uid = uid_expr_for(settings);
    let node = uid.render("c");
    // Self-link blocked pairs: each record paired with itself.
    let mut bp = CtePipeline::new().with_input(concat);
    bp.enqueue(
        format!(
            "select '0' as match_key, {node} as join_key_l, {node} as join_key_r from {} as c",
            concat.templated
        ),
        BLOCKED_ID_PAIRS,
    );
    let blocked = session.materialize(&bp)?;
    let pred_pipeline = predict::predict_pipeline(settings, concat, tf_tables, &blocked, None)?;
    let self_link = session.materialize(&pred_pipeline)?;

    let mut p = CtePipeline::new().with_input(&self_link);
    p.enqueue(
        format!(
            "select round(match_weight, 2) as match_weight, round(match_probability, 5) as match_probability from {}",
            self_link.templated
        ),
        "__splink__df_round_self_link",
    );
    p.enqueue(
        "select max(match_weight) as match_weight, match_probability, count(*) / cast(sum(count(*)) over () as float) as prop\nfrom __splink__df_round_self_link\ngroup by match_probability".to_string(),
        "__splink__df_unlinkables_proportions",
    );
    p.enqueue(
        "select *, sum(prop) over (order by match_probability) as cum_prop\nfrom __splink__df_unlinkables_proportions\nwhere match_probability < 1".to_string(),
        "__splink__df_unlinkables",
    );
    let out = session.materialize(&p)?;
    session.drop_table(&blocked)?;
    session.drop_table(&self_link)?;
    Ok(out)
}

/// False positives / false negatives vs a ground-truth label column.
/// Returns the labelled
/// predictions filtered to errors (with `clerical_match_score` and
/// `found_by_blocking_rules`). FN includes true matches missed by blocking.
#[allow(clippy::too_many_arguments)]
pub fn prediction_errors_from_labels_column(
    session: &Session,
    settings: &Settings,
    concat: &TableHandle,
    tf_tables: &[TableHandle],
    label_column: &str,
    threshold: f64,
    include_false_positives: bool,
    include_false_negatives: bool,
) -> Result<TableHandle> {
    let labelled = labelled_predictions(session, settings, concat, tf_tables, label_column)?;
    let t = pyrepr(threshold);
    let fp = format!("(clerical_match_score < {t} and match_probability > {t})");
    let fnn = format!(
        "((clerical_match_score > {t} and match_probability < {t}) or (clerical_match_score > {t} and found_by_blocking_rules = False))"
    );
    let mut conds = Vec::new();
    if include_false_positives {
        conds.push(fp);
    }
    if include_false_negatives {
        conds.push(fnn);
    }
    let where_clause = conds.join(" or ");
    let mut p = CtePipeline::new().with_input(&labelled);
    p.enqueue(
        format!("select * from {} where {where_clause}", labelled.templated),
        "__splink__predictions_from_label_column_fp_fn_only",
    );
    let out = session.materialize(&p)?;
    session.drop_table(&labelled)?;
    Ok(out)
}
