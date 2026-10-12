//! `__splink__df_match_weight_parts` (per-comparison `mw_` / `mw_tf_adj_`
//! CASE columns) and `__splink__df_predict` (summed match weight + stable
//! match probability). Matches goldens `predict_06___splink__df_match_weight_parts.sql`
//! and `predict_07___splink__df_predict.sql`.

use super::vectors::DF_COMPARISON_VECTORS;
use crate::comparison::Comparison;
use crate::error::Result;
use crate::scoring;
use crate::settings::Settings;
use crate::sql::literals::pyrepr;

pub const DF_MATCH_WEIGHT_PARTS: &str = "__splink__df_match_weight_parts";
pub const DF_PREDICT: &str = "__splink__df_predict";

/// The `mw_<name>` CASE: one WHEN arm per level in declaration order (the
/// ELSE level gets a `WHEN gamma = 0` arm too — no SQL ELSE clause). Null
/// levels contribute `cast(0.0 as float8)`.
pub fn mw_case_sql(c: &Comparison) -> Result<String> {
    let gamma = c.gamma_column_name();
    let mut arms = Vec::new();
    for level in &c.levels {
        if level.is_null_level {
            arms.push(format!("    WHEN {gamma} = -1 THEN cast(0.0 as float8)"));
        } else {
            let (m, u) = scoring::level_m_u(c, level)?;
            let mw = scoring::match_weight(m, u);
            arms.push(format!(
                "    WHEN {gamma} = {} THEN cast({} as float8)",
                level.gamma,
                pyrepr(mw)
            ));
        }
    }
    Ok(format!(
        "CASE\n{}\nEND as {}",
        arms.join("\n"),
        c.mw_column_name()
    ))
}

/// The `mw_tf_adj_<name>` CASE. Levels without a TF adjustment (and null /
/// ELSE / weight-0 levels) contribute `cast(0 as float8)`; TF levels emit
/// `weight * (log2(u_exact) - log2(greatest(coalesce(tf_l, tf_r),
/// coalesce(tf_r, tf_l), min_u)))`, guarded on TF existence.
pub fn mw_tf_adj_case_sql(c: &Comparison, tf_prefix: &str) -> Result<String> {
    let gamma = c.gamma_column_name();
    let mut arms = Vec::new();
    for level in &c.levels {
        let zero_arm = |g: i32| format!("    WHEN {gamma} = {g} THEN cast(0 as float8)");
        if level.is_null_level {
            arms.push(zero_arm(-1));
            continue;
        }
        let Some(tf) = &level.tf else {
            arms.push(zero_arm(level.gamma));
            continue;
        };
        if level.is_else || tf.weight == 0.0 {
            arms.push(zero_arm(level.gamma));
            continue;
        }
        let u_exact = scoring::u_probability_for_tf_base(c, level)?;
        let log2_u = pyrepr(u_exact.log2());
        let tf_l = format!("\"{tf_prefix}{}_l\"", tf.column);
        let tf_r = format!("\"{tf_prefix}{}_r\"", tf.column);
        let coalesce_lr = format!("coalesce({tf_l}, {tf_r})");
        let coalesce_rl = format!("coalesce({tf_r}, {tf_l})");
        let min_val = format!("cast({} as float8)", pyrepr(tf.minimum_u_value));
        arms.push(format!(
            "    WHEN {gamma} = {g} THEN\n        CASE\n            WHEN {coalesce_lr} is not null THEN\n                cast({weight} as float8) * (\n                    cast({log2_u} as float8) - log2(greatest({coalesce_lr}, {coalesce_rl}, {min_val}))\n                )\n            ELSE cast(0 as float8)\n        END",
            g = level.gamma,
            weight = pyrepr(tf.weight),
        ));
    }
    Ok(format!(
        "CASE\n{}\nEND as {}",
        arms.join("\n"),
        c.mw_tf_adj_column_name()
    ))
}

fn uid_pass_through(settings: &Settings) -> Vec<String> {
    let mut cols = Vec::new();
    if settings.source_dataset_required() {
        cols.push(format!("\"{}_l\"", settings.source_dataset_column));
        cols.push(format!("\"{}_r\"", settings.source_dataset_column));
    }
    cols.push(format!("\"{}_l\"", settings.unique_id_column));
    cols.push(format!("\"{}_r\"", settings.unique_id_column));
    cols
}

/// Select list for `__splink__df_match_weight_parts`.
pub fn match_weight_parts_sql(settings: &Settings) -> Result<String> {
    let mut cols = uid_pass_through(settings);
    for c in &settings.comparisons {
        if settings.retain_matching_columns {
            for col in &c.input_columns {
                cols.push(format!("\"{col}_l\""));
                cols.push(format!("\"{col}_r\""));
            }
        }
        cols.push(c.gamma_column_name());
        if settings.retain_intermediate {
            for col in c.tf_input_columns() {
                cols.push(format!("\"{}{col}_l\"", settings.tf_prefix));
                cols.push(format!("\"{}{col}_r\"", settings.tf_prefix));
            }
        }
        cols.push(mw_case_sql(c)?);
        if c.has_tf_adjustments() {
            cols.push(mw_tf_adj_case_sql(c, &settings.tf_prefix)?);
        }
    }
    for col in &settings.additional_columns_to_retain {
        cols.push(format!("\"{col}_l\""));
        cols.push(format!("\"{col}_r\""));
    }
    cols.push("match_key".to_string());
    Ok(format!(
        "select\n    {}\nfrom {DF_COMPARISON_VECTORS}",
        cols.join(",\n    ")
    ))
}

/// The summed match-weight expression: prior log2-odds + every `mw_` /
/// `mw_tf_adj_` column in comparison order.
pub fn match_weight_expr(settings: &Settings, prior: f64) -> String {
    let mut terms = vec![format!(
        "cast({} as float8)",
        pyrepr(scoring::prior_log2_odds(prior))
    )];
    for c in &settings.comparisons {
        terms.push(c.mw_column_name());
        if c.has_tf_adjustments() {
            terms.push(c.mw_tf_adj_column_name());
        }
    }
    terms.join(" + ")
}

/// Final `__splink__df_predict` select. `training_mode` keeps gamma columns
/// even when matching columns are not retained (used by EM's E-step).
/// `threshold_match_weight` filters rows.
pub fn predict_sql(
    settings: &Settings,
    prior: f64,
    training_mode: bool,
    threshold_match_weight: Option<f64>,
) -> Result<String> {
    let mw = match_weight_expr(settings, prior);
    let match_prob = format!(
        "CASE\n        WHEN ({mw}) >= 0 THEN\n            1.0 / (1.0 + POWER(cast(2 as float8), -({mw})))\n        ELSE\n            POWER(cast(2 as float8), ({mw})) /\n            (1.0 + POWER(cast(2 as float8), ({mw})))\n    END as match_probability"
    );

    let mut cols = vec![format!("{mw} as match_weight"), match_prob];
    cols.extend(uid_pass_through(settings));
    for c in &settings.comparisons {
        if settings.retain_matching_columns {
            for col in &c.input_columns {
                cols.push(format!("\"{col}_l\""));
                cols.push(format!("\"{col}_r\""));
            }
        }
        if settings.retain_matching_columns || training_mode {
            cols.push(c.gamma_column_name());
        }
        if settings.retain_intermediate {
            for col in c.tf_input_columns() {
                cols.push(format!("\"{}{col}_l\"", settings.tf_prefix));
                cols.push(format!("\"{}{col}_r\"", settings.tf_prefix));
            }
            cols.push(c.mw_column_name());
            if c.has_tf_adjustments() {
                cols.push(c.mw_tf_adj_column_name());
            }
        }
    }
    for col in &settings.additional_columns_to_retain {
        cols.push(format!("\"{col}_l\""));
        cols.push(format!("\"{col}_r\""));
    }
    cols.push("match_key".to_string());

    let where_clause = match threshold_match_weight {
        Some(t) => format!("\nwhere ({mw}) >= {}", pyrepr(t)),
        None => String::new(),
    };
    Ok(format!(
        "select\n    {}\nfrom {DF_MATCH_WEIGHT_PARTS}{where_clause}",
        cols.join(",\n    ")
    ))
}
