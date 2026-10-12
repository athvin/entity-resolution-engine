//! `__splink__df_concat`: vertical concatenation of input tables.
//! (v1 exercises the single-input dedupe path; the multi-input UNION ALL with
//! a `source_dataset` literal follows the same shape.)

use crate::session::TableHandle;

pub const DF_CONCAT: &str = "__splink__df_concat";

/// Single input table: a plain projection of its columns, in table order.
/// Matches golden `predict_00___splink__df_concat.sql`.
pub fn concat_sql_single(input: &TableHandle) -> String {
    let cols = input
        .columns
        .iter()
        .map(|c| format!("\"{c}\""))
        .collect::<Vec<_>>()
        .join(",\n    ");
    format!("select\n    {cols}\nfrom {}", input.templated)
}

/// Multiple inputs (link types): UNION ALL, each arm prefixed with its
/// dataset display name as the `source_dataset` literal unless the column
/// already exists. Column order comes from the first table.
///
/// Inputs are referenced by PHYSICAL name (not templated), so the dataset
/// display-name string literals (e.g. `'df_a'`) are never mangled by the
/// pipeline's templated→physical identifier rewriting.
pub fn concat_sql_union(inputs: &[(&TableHandle, &str)], source_dataset_column: &str) -> String {
    let first_cols = &inputs[0].0.columns;
    let has_source_col = first_cols.iter().any(|c| c == source_dataset_column);
    let arms: Vec<String> = inputs
        .iter()
        .map(|(handle, display_name)| {
            let cols = first_cols
                .iter()
                .map(|c| format!("\"{c}\""))
                .collect::<Vec<_>>()
                .join(",\n    ");
            if has_source_col {
                format!("select\n    {cols}\nfrom {}", handle.physical)
            } else {
                format!(
                    "select\n    '{display_name}' as {source_dataset_column},\n    {cols}\nfrom {}",
                    handle.physical
                )
            }
        })
        .collect();
    arms.join("\n\nUNION ALL\n\n")
}
