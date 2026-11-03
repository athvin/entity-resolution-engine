//! Data profiling: column completeness, value-frequency distributions, and the
//! per-column shape a corpus has to be understood by before it is configured.
//! Backend data only — no charts.

use crate::error::{DedupeError, Result};
use crate::session::{Session, TableHandle};
use crate::settings::Settings;
use crate::sql::pipeline::CtePipeline;
use crate::training::estimate_u::deterministic_sample_predicate;

/// Quote an identifier, doubling any embedded quote.
///
/// One spelling for the whole module: the per-column SQL here interpolates a
/// column name into both an identifier and a string literal, and three inline
/// variants of the same escape is how one of them ends up missing.
fn quoted_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// The materialized per-column shape table.
pub const COLUMN_SHAPE: &str = "__dedupe__column_shape";

/// A deterministic row sample for a profile.
///
/// Deterministic rather than random because a profile is compared: against the
/// previous delivery to find drift, and against its own golden to prove the
/// profiler did not change. A sample drawn differently on each run makes both
/// comparisons meaningless, so this reuses the same hash-bucket scheme u
/// estimation samples with.
#[derive(Debug, Clone)]
pub struct ProfileSample {
    /// The row-id expression to hash. Any expression unique per row; a profile
    /// of delivered rows has no model to supply one.
    pub id_expr: String,
    /// Proportion in `(0, 1]`. At `>= 1.0` no filter is emitted.
    pub proportion: f64,
    /// Salt. The same seed always draws the same rows.
    pub seed: Option<i64>,
}

/// What to compute for each column.
#[derive(Debug, Clone, Default)]
pub struct ProfileOptions {
    /// Also compute EXACT distinct counts (raw and whitespace/case-normalized).
    ///
    /// Off by default because `count(distinct)` is a per-column sort where
    /// `approx_count_distinct` is a sketch, and the approximate figure is
    /// always reported. Turn it on when the answer has to be trusted as a
    /// uniqueness verdict — see [`ColumnShape::is_unique`], which declines to
    /// answer without it.
    pub exact_distinct: bool,
    /// Sample rows rather than reading all of them.
    pub sample: Option<ProfileSample>,
}

/// One column's shape.
#[derive(Debug, Clone, PartialEq)]
pub struct ColumnShape {
    pub column_name: String,
    /// Rows the profile READ (after sampling).
    pub row_count: i64,
    /// Rows in the corpus, whether or not they were sampled.
    pub total_row_count: i64,
    pub non_null_count: i64,
    /// Present but empty once trimmed.
    ///
    /// Counted apart from NULL because ingest lands every delivered value as a
    /// JSON *string*, so an empty cell arrives as `''` and is not NULL. A fill
    /// rate that treats the two alike reports a column as populated when every
    /// value in it is blank.
    pub blank_count: i64,
    pub distinct_approx: i64,
    pub distinct_exact: Option<i64>,
    /// Distinct values after lowercasing, trimming and collapsing internal
    /// whitespace. Materially below `distinct_exact` means the column needs
    /// standardizing before it is compared.
    pub distinct_normalized: Option<i64>,
    pub len_min: Option<i64>,
    pub len_max: Option<i64>,
    pub len_avg: Option<f64>,
    pub len_p50: Option<i64>,
    pub len_p95: Option<i64>,
    pub lex_min: Option<String>,
    pub lex_max: Option<String>,
    pub num_min: Option<f64>,
    pub num_max: Option<f64>,
    pub num_avg: Option<f64>,
    pub num_std: Option<f64>,
    /// Non-null values that parse as each type. The declared type says nothing
    /// here — everything arrives as text — so conformance is the only evidence
    /// of what a column actually holds.
    pub cast_bigint: i64,
    pub cast_double: i64,
    pub cast_date: i64,
    pub cast_timestamp: i64,
    pub cast_boolean: i64,
    /// Shannon entropy of the value distribution, in bits.
    pub entropy_bits: Option<f64>,
}

impl ColumnShape {
    /// Non-null, non-blank proportion of the rows read. `None` for an empty
    /// read, where a fill rate has no meaning and `0.0` would assert one.
    #[must_use]
    pub fn fill_rate(&self) -> Option<f64> {
        if self.row_count == 0 {
            return None;
        }
        Some((self.non_null_count - self.blank_count) as f64 / self.row_count as f64)
    }

    /// Distinct values per non-null value: ~1.0 is a candidate identifier,
    /// ~0.0 a constant. Uses the exact count when it was computed, because the
    /// sketch can exceed the row count and push this above 1.0.
    ///
    /// Clamped to `1.0`, so this must NOT be used to decide uniqueness — see
    /// [`ColumnShape::is_unique`].
    #[must_use]
    pub fn cardinality_ratio(&self) -> Option<f64> {
        if self.non_null_count == 0 {
            return None;
        }
        let distinct = self.distinct_exact.unwrap_or(self.distinct_approx);
        Some((distinct as f64 / self.non_null_count as f64).min(1.0))
    }

    /// Whether every non-null value is distinct — the "is this a usable record
    /// id?" question.
    ///
    /// `None` when the exact distinct count was not computed, and that is the
    /// whole reason this exists rather than a `cardinality_ratio() == 1.0`
    /// comparison.
    ///
    /// `approx_count_distinct` is a HyperLogLog sketch and is wrong in BOTH
    /// directions, each breaking the check differently. Measured on fake_1000's
    /// `unique_id`, which holds 1000 genuinely distinct values, it returns
    /// **978** — an undershoot, so the ratio is 0.978, never reaches the clamp,
    /// and a ratio-based check would call a perfectly unique column
    /// non-unique. An overshoot does the reverse: the ratio exceeds 1.0, gets
    /// clamped to exactly 1.0, and real duplicates vanish behind it. Since
    /// catching those duplicates is the entire purpose of the check, declining
    /// to answer without an exact count is the only safe behaviour.
    #[must_use]
    pub fn is_unique(&self) -> Option<bool> {
        let exact = self.distinct_exact?;
        if self.non_null_count == 0 {
            return None;
        }
        Some(exact == self.non_null_count)
    }
}

/// Per-column shape over `table`, one row per column, materialized as
/// [`COLUMN_SHAPE`]. Reads only.
///
/// Every statistic is taken through `cast(col as varchar)` rather than the
/// declared type. That is not laziness: it is the only form that binds for
/// every column of every type (a `try_cast` from DATE to DOUBLE is a bind
/// error, not a NULL), and DuckDB's double/varchar conversion is
/// shortest-round-trippable, so nothing is lost by going through text.
///
/// # Errors
/// Propagates the lake read. Refuses an empty column list rather than emitting
/// invalid SQL.
pub fn column_shape(
    session: &Session,
    table: &TableHandle,
    columns: &[String],
    options: &ProfileOptions,
) -> Result<TableHandle> {
    if columns.is_empty() {
        return Err(DedupeError::settings(
            "column_shape needs at least one column",
        ));
    }
    let where_clause = options
        .sample
        .as_ref()
        .and_then(|s| deterministic_sample_predicate(&s.id_expr, s.proportion, s.seed))
        .map_or(String::new(), |p| format!(" where {p}"));

    let arms: Vec<String> = columns
        .iter()
        .map(|c| shape_arm(c, table.templated.as_str(), &where_clause, options))
        .collect();

    let mut p = CtePipeline::new().with_input(table);
    p.enqueue(arms.join("\n\nUNION ALL\n\n"), COLUMN_SHAPE);
    session.materialize(&p)
}

/// One column's `SELECT` arm. Every arm must project the same columns in the
/// same order, because they are `UNION ALL`-ed.
fn shape_arm(column: &str, table: &str, where_clause: &str, options: &ProfileOptions) -> String {
    let q = quoted_ident(column);
    let lit = column.replace('\'', "''");
    let v = format!("cast({q} as varchar)");
    // Length and lexical statistics are over the TRIMMED text, so a padded
    // value is not reported as a longer one.
    let t = format!("trim({v})");
    let (distinct_exact, distinct_norm) = if options.exact_distinct {
        (
            format!("cast(count(distinct {q}) as bigint)"),
            format!(
                "cast(count(distinct lower(trim(regexp_replace({v}, '\\s+', ' ', 'g')))) as bigint)"
            ),
        )
    } else {
        (
            "cast(null as bigint)".to_string(),
            "cast(null as bigint)".to_string(),
        )
    };

    format!(
        "select\n  \
           '{lit}' as column_name,\n  \
           cast(count(*) as bigint) as row_count,\n  \
           cast((select count(*) from {table}) as bigint) as total_row_count,\n  \
           cast(count({q}) as bigint) as non_null_count,\n  \
           cast(count(*) filter (where {q} is not null and {t} = '') as bigint) as blank_count,\n  \
           cast(approx_count_distinct({q}) as bigint) as distinct_approx,\n  \
           {distinct_exact} as distinct_exact,\n  \
           {distinct_norm} as distinct_normalized,\n  \
           cast(min(length({t})) as bigint) as len_min,\n  \
           cast(max(length({t})) as bigint) as len_max,\n  \
           cast(avg(length({t})) as double) as len_avg,\n  \
           cast(approx_quantile(length({t}), 0.5) as bigint) as len_p50,\n  \
           cast(approx_quantile(length({t}), 0.95) as bigint) as len_p95,\n  \
           cast(min({t}) as varchar) as lex_min,\n  \
           cast(max({t}) as varchar) as lex_max,\n  \
           cast(min(try_cast({t} as double)) as double) as num_min,\n  \
           cast(max(try_cast({t} as double)) as double) as num_max,\n  \
           cast(avg(try_cast({t} as double)) as double) as num_avg,\n  \
           cast(stddev_samp(try_cast({t} as double)) as double) as num_std,\n  \
           cast(count(try_cast({t} as bigint)) as bigint) as cast_bigint,\n  \
           cast(count(try_cast({t} as double)) as bigint) as cast_double,\n  \
           cast(count(try_cast({t} as date)) as bigint) as cast_date,\n  \
           cast(count(try_cast({t} as timestamp)) as bigint) as cast_timestamp,\n  \
           cast(count(try_cast({t} as boolean)) as bigint) as cast_boolean,\n  \
           cast(entropy({q}) as double) as entropy_bits\n\
         from {table}{where_clause}"
    )
}

/// One value's share of a column.
#[derive(Debug, Clone, PartialEq)]
pub struct ValueFrequency {
    pub column_name: String,
    /// The value as text. `None` only for a value that is genuinely empty
    /// text, which [`ColumnShape::blank_count`] reports as a count.
    pub value: Option<String>,
    pub count: i64,
    /// `count` over the column's non-null rows.
    pub share: f64,
    /// 1-based, most frequent first. Rank 1 is the column's mode.
    pub rank: i64,
}

/// The `k` most frequent values of each column, and nothing else.
///
/// [`column_value_frequencies`] answers a wider question — every distinct value
/// with its count — and is the wrong tool for a profile: it materializes one row
/// per distinct value, so finding the top ten of a high-cardinality email column
/// over ten million rows builds a ten-million-row table to read ten rows from.
/// This pushes the `LIMIT` into SQL, so the work is bounded by `k` per column
/// whatever the corpus is.
///
/// Ties break on the value itself, not arbitrarily, because an unstable order
/// would make two profiles of identical data differ.
///
/// # Errors
/// Propagates the lake read. Refuses an empty column list.
pub fn top_values(
    session: &Session,
    table: &TableHandle,
    columns: &[String],
    k: usize,
    options: &ProfileOptions,
) -> Result<Vec<ValueFrequency>> {
    if columns.is_empty() {
        return Err(DedupeError::settings(
            "top_values needs at least one column",
        ));
    }
    let sample = options
        .sample
        .as_ref()
        .and_then(|s| deterministic_sample_predicate(&s.id_expr, s.proportion, s.seed));

    let mut out = Vec::new();
    for column in columns {
        let q = quoted_ident(column);
        let v = format!("cast({q} as varchar)");
        let mut filter = format!("{q} is not null");
        if let Some(p) = &sample {
            filter = format!("{filter} and {p}");
        }
        let sql = format!(
            "select {v} as value, cast(count(*) as bigint) as n,\n  \
               cast(count(*) * 1.0 / nullif((select count({q}) from {t} where {filter}), 0) as double) as share\n\
             from {t} where {filter}\n\
             group by {v}\n\
             order by n desc, value asc\n\
             limit {k}",
            t = table.physical,
        );
        for (i, row) in session.query_strings(&sql)?.into_iter().enumerate() {
            out.push(ValueFrequency {
                column_name: column.clone(),
                value: if row[0].is_empty() {
                    None
                } else {
                    Some(row[0].clone())
                },
                count: row[1].parse().unwrap_or(0),
                share: row[2].parse().unwrap_or(0.0),
                rank: i as i64 + 1,
            });
        }
    }
    Ok(out)
}

/// The most frequent value of each column, keyed by column name.
///
/// The mode's SHARE is the number a blocking decision turns on: an equi-join on
/// a key whose top value covers share `s` of the corpus generates on the order
/// of `(s * n)^2 / 2` pairs from that one value alone, so a key can look
/// affordable in aggregate and still be dominated by its mode.
///
/// # Errors
/// Propagates the lake read.
pub fn modes(
    session: &Session,
    table: &TableHandle,
    columns: &[String],
    options: &ProfileOptions,
) -> Result<Vec<ValueFrequency>> {
    top_values(session, table, columns, 1, options)
}

/// Read a materialized [`column_shape`] table into typed rows, ordered by
/// column name so the result is stable.
///
/// # Errors
/// Propagates the lake read.
pub fn read_column_shapes(session: &Session, shape: &TableHandle) -> Result<Vec<ColumnShape>> {
    let rows = session.query_strings(&format!(
        "select column_name, row_count, total_row_count, non_null_count, blank_count,\n  \
           distinct_approx, distinct_exact, distinct_normalized,\n  \
           len_min, len_max, len_avg, len_p50, len_p95, lex_min, lex_max,\n  \
           num_min, num_max, num_avg, num_std,\n  \
           cast_bigint, cast_double, cast_date, cast_timestamp, cast_boolean, entropy_bits\n\
         from {} order by column_name",
        shape.physical
    ))?;
    // `query_strings` renders SQL NULL as an empty string, so "absent" and
    // "empty text" are the same token here. Only `lex_min`/`lex_max` can
    // legitimately be empty text, and for them an empty column is the honest
    // answer either way.
    let opt = |s: &str| -> Option<String> {
        if s.is_empty() {
            None
        } else {
            Some(s.to_string())
        }
    };
    let int = |s: &str| -> i64 { s.parse().unwrap_or(0) };
    let opt_int = |s: &str| -> Option<i64> { s.parse().ok() };
    let opt_f = |s: &str| -> Option<f64> { s.parse().ok() };

    Ok(rows
        .into_iter()
        .map(|r| ColumnShape {
            column_name: r[0].clone(),
            row_count: int(&r[1]),
            total_row_count: int(&r[2]),
            non_null_count: int(&r[3]),
            blank_count: int(&r[4]),
            distinct_approx: int(&r[5]),
            distinct_exact: opt_int(&r[6]),
            distinct_normalized: opt_int(&r[7]),
            len_min: opt_int(&r[8]),
            len_max: opt_int(&r[9]),
            len_avg: opt_f(&r[10]),
            len_p50: opt_int(&r[11]),
            len_p95: opt_int(&r[12]),
            lex_min: opt(&r[13]),
            lex_max: opt(&r[14]),
            num_min: opt_f(&r[15]),
            num_max: opt_f(&r[16]),
            num_avg: opt_f(&r[17]),
            num_std: opt_f(&r[18]),
            cast_bigint: int(&r[19]),
            cast_double: int(&r[20]),
            cast_date: int(&r[21]),
            cast_timestamp: int(&r[22]),
            cast_boolean: int(&r[23]),
            entropy_bits: opt_f(&r[24]),
        })
        .collect())
}

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
            // The column name reaches the SQL twice, as a literal and as an
            // identifier, and each needs its own escape. The names here come
            // from an operator's `--column` rather than from delivered data, so
            // this is robustness rather than a security boundary — but the
            // functions added to this module alongside these two do escape, and
            // leaving one convention per function means a reader cannot tell
            // which applies. A quote in a name produced invalid SQL and a
            // confusing parse error rather than anything worse.
            let q = quoted_ident(c);
            let lit = c.replace('\'', "''");
            format!(
                "select {group_select}, '{lit}' as column_name,\n  count(*) - count({q}) as total_null_rows,\n  count(*) as total_rows_inc_nulls,\n  cast(count({q})*1.0/count(*) as float) as completeness\nfrom {concat} {group_by}",
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
            let q = quoted_ident(c);
            let lit = c.replace('\'', "''");
            format!(
                "select count(*) as value_count, '{lit}' as group_name, cast({q} as varchar) as value,\n  (select count({q}) from {concat}) as total_non_null_rows,\n  (select count(*) from {concat}) as total_rows_inc_nulls,\n  (select count(distinct {q}) from {concat}) as distinct_value_count\nfrom {concat} where {q} is not null group by {q}",
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
