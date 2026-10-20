//! Term-frequency tables and the `__dedupe__concat_with_tf` join.
//!
//! TF tables are the materialized/cached artifacts in v5;
//! `__dedupe__concat_with_tf` is a CTE re-derived per pipeline.

use super::concat::DF_CONCAT;

pub const DF_CONCAT_WITH_TF: &str = "__dedupe__concat_with_tf";

pub fn tf_table_name(column: &str) -> String {
    format!("__dedupe__tf_{column}")
}

/// Matches golden `training_23___dedupe__tf_first_name.sql`:
/// denominator is `count(col)` (non-null count), numerator `count(*)` per
/// value, NULL values excluded.
pub fn tf_table_sql(column: &str, tf_prefix: &str) -> String {
    format!(
        "select\n\"{column}\",\ncast(count(*) as float8) / (\n    select count(\"{column}\") as total\n    from {DF_CONCAT}\n) as \"{tf_prefix}{column}\"\nfrom {DF_CONCAT}\nwhere \"{column}\" is not null\ngroup by \"{column}\""
    )
}

/// Matches golden `predict_03___dedupe__concat_with_tf.sql`: left-join each
/// TF table onto the concat on the raw column. With no TF columns this
/// degenerates to `select * from __dedupe__concat`.
pub fn concat_with_tf_sql(tf_columns: &[String], tf_prefix: &str) -> String {
    concat_with_tf_sql_from(DF_CONCAT, tf_columns, tf_prefix)
}

/// As `concat_with_tf_sql`, but joining TF tables onto an arbitrary source
/// table (used by incremental linkage, where the left and right sides are
/// separate concats sharing the established TF tables).
pub fn concat_with_tf_sql_from(source: &str, tf_columns: &[String], tf_prefix: &str) -> String {
    if tf_columns.is_empty() {
        return format!("select * from {source}");
    }
    let select_tf = tf_columns
        .iter()
        .map(|c| format!("{}.\"{tf_prefix}{c}\"", tf_table_name(c)))
        .collect::<Vec<_>>()
        .join(",\n    ");
    let joins = tf_columns
        .iter()
        .map(|c| {
            let tf_tbl = tf_table_name(c);
            format!("left join {tf_tbl} on {source}.\"{c}\" = {tf_tbl}.\"{c}\"")
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!("select\n    {source}.*,\n    {select_tf}\nfrom {source}\n{joins}")
}
