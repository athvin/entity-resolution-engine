//! Data profiling: column completeness and value-frequency distributions.
//! Backend data only — no charts.

use crate::error::Result;
use crate::session::{Session, TableHandle};
use crate::settings::Settings;
use crate::sql::pipeline::CtePipeline;

/// Per-column completeness (non-null proportion), one row per column (and per
/// source dataset for link types); `completeness` is single-precision.
pub fn completeness(
    session: &Session,
    settings: &Settings,
    concat: &TableHandle,
    columns: &[String],
    dataset_name: &str,
) -> Result<TableHandle> {
    let (group_select, group_by) = if settings.source_dataset_required() {
        (
            format!("\"{}\" as source_dataset", settings.source_dataset_column),
            format!("group by \"{}\"", settings.source_dataset_column),
        )
    } else {
        (format!("'{dataset_name}' as source_dataset"), String::new())
    };
    let arms: Vec<String> = columns
        .iter()
        .map(|c| {
            format!(
                "select {group_select}, '{c}' as column_name,\n  count(*) - count(\"{c}\") as total_null_rows,\n  count(*) as total_rows_inc_nulls,\n  cast(count(\"{c}\")*1.0/count(*) as float) as completeness\nfrom {concat} {group_by}",
                concat = concat.templated
            )
        })
        .collect();
    let mut p = CtePipeline::new().with_input(concat);
    p.enqueue(arms.join("\n\nUNION ALL\n\n"), "__dedupe__completeness");
    session.materialize(&p)
}

/// Per-column value-frequency distribution: one row per distinct non-null
/// value with its count and per-column totals. `group_name` is the column.
pub fn column_value_frequencies(
    session: &Session,
    concat: &TableHandle,
    columns: &[String],
) -> Result<TableHandle> {
    let arms: Vec<String> = columns
        .iter()
        .map(|c| {
            format!(
                "select count(*) as value_count, '{c}' as group_name, cast(\"{c}\" as varchar) as value,\n  (select count(\"{c}\") from {concat}) as total_non_null_rows,\n  (select count(*) from {concat}) as total_rows_inc_nulls,\n  (select count(distinct \"{c}\") from {concat}) as distinct_value_count\nfrom {concat} where \"{c}\" is not null group by \"{c}\"",
                concat = concat.templated
            )
        })
        .collect();
    let mut p = CtePipeline::new().with_input(concat);
    p.enqueue(
        arms.join("\n\nUNION ALL\n\n"),
        "__dedupe__all_column_value_frequencies",
    );
    session.materialize(&p)
}
