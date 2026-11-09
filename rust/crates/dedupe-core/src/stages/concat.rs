//! `__dedupe__concat`: vertical concatenation of input tables.
//! (v1 exercises the single-input dedupe path; the multi-input UNION ALL with
//! a `source_dataset` literal follows the same shape.)

use crate::session::TableHandle;

pub const DF_CONCAT: &str = "__dedupe__concat";

/// Single input table: a plain projection of its columns, in table order.
/// Matches golden `predict_00___dedupe__concat.sql`.
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
                let display_name = display_name.replace('\'', "''");
                format!(
                    "select\n    '{display_name}' as {source_dataset_column},\n    {cols}\nfrom {}",
                    handle.physical
                )
            }
        })
        .collect();
    arms.join("\n\nUNION ALL\n\n")
}

#[cfg(test)]
mod tests {
    use crate::session::{Session, SessionOptions, TableHandle};

    use super::concat_sql_union;

    #[test]
    fn dataset_names_are_sql_values_and_keep_composite_ids_distinct() {
        let session = Session::open_in_memory(SessionOptions::deterministic()).unwrap();
        session
            .execute_batch("create table first (unique_id varchar); insert into first values ('1'); create table second (unique_id varchar); insert into second values ('1')")
            .unwrap();
        let first = TableHandle {
            templated: "first".to_string(),
            physical: "first".to_string(),
            columns: vec!["unique_id".to_string()],
        };
        let second = TableHandle {
            templated: "second".to_string(),
            physical: "second".to_string(),
            columns: vec!["unique_id".to_string()],
        };
        let sql = concat_sql_union(&[(&first, "O'Brien"), (&second, "plain")], "source_dataset");
        let rows = session
            .query_strings(&format!(
                "select source_dataset, source_dataset || '-__-' || unique_id from ({sql}) order by source_dataset"
            ))
            .unwrap();
        assert_eq!(
            rows,
            vec![
                vec!["O'Brien".to_string(), "O'Brien-__-1".to_string()],
                vec!["plain".to_string(), "plain-__-1".to_string()],
            ]
        );
    }
}
