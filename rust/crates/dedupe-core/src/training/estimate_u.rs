//! u estimation via deterministic random sampling.
//!
//! Deterministic hash sampling of records, then a per-comparison
//! "block-and-count" over the cartesian product of the sample with
//! RHS-only hash chunking, a probe pass, and early stopping once every level
//! has accumulated at least `min_count_per_level` observations. u for each
//! level is `count_level / sum(count over non-null levels)`; zero-count
//! levels become the not-observed sentinel. TF adjustments are disabled
//! during u estimation.

use std::collections::BTreeMap;

use crate::comparison::Comparison;
use crate::error::Result;
use crate::model::{EstimateSource, ProbEstimate, TrainedModel};
use crate::session::{Session, TableHandle};
use crate::settings::Settings;
use crate::sql::pipeline::CtePipeline;
use crate::stages::uid_expr_for;
use crate::stages::vectors::gamma_case_sql;

const DETERMINISTIC_SAMPLE_MODULUS: i64 = 1_000_000_000;

#[derive(Debug, Clone, Copy)]
pub struct UOptions {
    pub max_pairs: f64,
    pub seed: Option<i64>,
    /// None disables early stopping (all chunks always run).
    pub min_count_per_level: Option<i64>,
    pub num_chunks: u32,
}

impl Default for UOptions {
    fn default() -> Self {
        UOptions {
            max_pairs: 1e6,
            seed: None,
            min_count_per_level: Some(100),
            num_chunks: 10,
        }
    }
}

/// Number of rows whose cartesian product yields ~n_pairs pairs.
// `f64::midpoint` is not bit-identical to `0.5 * (a + b)`; this expression must
// match the reference sample-size arithmetic exactly, so the clippy suggestion
// (manual_midpoint, pedantic) is refused rather than applied.
#[allow(clippy::manual_midpoint)]
fn rows_needed_for_n_pairs(n_pairs: f64) -> f64 {
    0.5 * ((8.0 * n_pairs + 1.0).sqrt() + 1.0)
}

/// Python's `round` (banker's rounding / round-half-to-even).
fn round_half_even(x: f64) -> f64 {
    let floor = x.floor();
    let diff = x - floor;
    if diff < 0.5 {
        floor
    } else if diff > 0.5 {
        floor + 1.0
    } else if (floor as i64) % 2 == 0 {
        floor
    } else {
        floor + 1.0
    }
}

/// Unquoted composite unique-id expression (no table prefix), used by the
/// sample filter. Matches `_composite_unique_id_from_nodes_sql`.
fn uid_sql_unprefixed(settings: &Settings) -> String {
    if settings.source_dataset_required() {
        format!(
            "\"{}\" || '-__-' || \"{}\"",
            settings.source_dataset_column, settings.unique_id_column
        )
    } else {
        format!("\"{}\"", settings.unique_id_column)
    }
}

/// Composite unique-id expression with a table prefix, used by the chunk
/// filter.
fn uid_sql_prefixed(settings: &Settings, prefix: &str) -> String {
    if settings.source_dataset_required() {
        format!(
            "{prefix}.\"{}\" || '-__-' || {prefix}.\"{}\"",
            settings.source_dataset_column, settings.unique_id_column
        )
    } else {
        format!("{prefix}.\"{}\"", settings.unique_id_column)
    }
}

/// The sample `WHERE` filter (empty when proportion >= 1.0). Matches
/// `proportion_sample_sql` with the DuckDB `hash(x) % m` bucket.
fn sample_filter_sql(settings: &Settings, proportion: f64, seed: Option<i64>) -> String {
    if proportion >= 1.0 {
        return String::new();
    }
    let mut id = uid_sql_unprefixed(settings);
    if let Some(s) = seed {
        id = format!("({id}) || '_{s}'");
    }
    let threshold =
        (round_half_even(proportion * DETERMINISTIC_SAMPLE_MODULUS as f64) as i64).max(1);
    format!(" AND hash({id}) % {DETERMINISTIC_SAMPLE_MODULUS} < {threshold}")
}

/// RHS-only chunk filter (empty when num_chunks == 1). Matches
/// `_chunk_assignment_sql`: `(ABS(hash(r.<uid>)) % n) + 1 = chunk`.
fn chunk_filter_sql(settings: &Settings, chunk_num: u32, num_chunks: u32) -> String {
    if num_chunks == 1 {
        return String::new();
    }
    let id = uid_sql_prefixed(settings, "r");
    format!(" AND (ABS(hash({id})) % {num_chunks}) + 1 = {chunk_num}")
}

/// Build the per-chunk counting SQL: `(cvv, u_count)` for one comparison over
/// the sample, with the given chunk filter. `match_probability` is pinned to
/// 0, so `u_count = count(*)` per gamma.
fn count_pipeline(
    settings: &Settings,
    comparison: &Comparison,
    sample: &TableHandle,
    chunk_num: u32,
    num_chunks: u32,
) -> CtePipeline {
    let uid = uid_expr_for(settings);
    let uid_l = uid.render("l");
    let uid_r = uid.render("r");

    // Blocking cols: the comparison's input columns as l/r.
    let mut blocking_cols = Vec::new();
    for col in &comparison.input_columns {
        blocking_cols.push(format!("l.\"{col}\" as \"{col}_l\""));
        blocking_cols.push(format!("r.\"{col}\" as \"{col}_r\""));
    }
    let blocking_cols = blocking_cols.join(",\n");

    // Dedup/cross condition by link type: link_only keeps only cross-dataset
    // pairs (the two_dataset split produces the same pair set).
    let where_dedupe = if settings.link_type == crate::settings::json::LinkType::LinkOnly {
        let sd = &settings.source_dataset_column;
        format!("where {uid_l} < {uid_r} and l.\"{sd}\" != r.\"{sd}\"")
    } else {
        format!("where {uid_l} < {uid_r}")
    };
    let chunk = chunk_filter_sql(settings, chunk_num, num_chunks);
    let blocked_sql = format!(
        "select\n{uid_l} as join_key_l,\n{uid_r} as join_key_r,\n{blocking_cols},\n'0' as match_key\nfrom {sample} as l\ninner join {sample} as r\non\n(1=1)\n{where_dedupe}{chunk}",
        sample = sample.templated,
    );

    let gamma = gamma_case_sql(comparison);
    let cv_sql = format!("select\n{gamma}\nfrom __dedupe__blocked_id_pairs");

    let counts_sql = format!(
        "select {g} as comparison_vector_value, count(*) as u_count\nfrom __dedupe__comparison_vectors\ngroup by {g}",
        g = comparison.gamma_column_name()
    );

    let mut p = CtePipeline::new().with_input(sample);
    p.enqueue(blocked_sql, "__dedupe__blocked_id_pairs");
    p.enqueue(cv_sql, "__dedupe__comparison_vectors");
    p.enqueue(counts_sql, "__dedupe__m_u_counts");
    p
}

/// Accumulated u-counts per gamma for one comparison (non-null levels only).
struct Accumulator {
    counts: BTreeMap<i32, f64>,
}

impl Accumulator {
    fn new(comparison: &Comparison) -> Self {
        let counts = comparison
            .levels
            .iter()
            .filter(|l| !l.is_null_level)
            .map(|l| (l.gamma, 0.0))
            .collect();
        Accumulator { counts }
    }

    fn update(&mut self, chunk_counts: &[(i32, f64)]) {
        for (cvv, u) in chunk_counts {
            if let Some(acc) = self.counts.get_mut(cvv) {
                *acc += u;
            }
        }
    }

    fn min_count(&self) -> f64 {
        self.counts.values().copied().fold(f64::INFINITY, f64::min)
    }

    fn all_meet(&self, min_count: i64) -> bool {
        self.min_count() >= min_count as f64
    }
}

fn run_chunk(
    session: &Session,
    settings: &Settings,
    comparison: &Comparison,
    sample: &TableHandle,
    chunk_num: u32,
    num_chunks: u32,
) -> Result<Vec<(i32, f64)>> {
    let pipeline = count_pipeline(settings, comparison, sample, chunk_num, num_chunks);
    let counts = session.materialize(&pipeline)?;
    let rows = session.query_strings(&format!(
        "select comparison_vector_value, u_count from {}",
        counts.physical
    ))?;
    session.drop_table(&counts)?;
    Ok(rows
        .into_iter()
        .map(|r| (r[0].parse::<i32>().unwrap(), r[1].parse::<f64>().unwrap()))
        .collect())
}

/// Estimate u for every comparison and write the combined values into the
/// model. `concat` is `__dedupe__concat`.
pub fn estimate_u(
    session: &Session,
    model: &mut TrainedModel,
    concat: &TableHandle,
    opts: UOptions,
) -> Result<()> {
    assert!(opts.num_chunks >= 1, "num_chunks must be >= 1");
    let settings = &model.settings;

    let total_nodes = session.count(&concat.physical)? as f64;
    // Sampling proportion by link type: dedupe/link_and_dedupe invert
    // the pairs-from-rows formula; link_only uses sqrt(max_pairs / total_links)
    // over the pairwise cross-dataset total.
    let mut proportion = if settings.link_type == crate::settings::json::LinkType::LinkOnly {
        let rows = session.query_strings(&format!(
            "select count(*) from {} group by \"{}\"",
            concat.physical, settings.source_dataset_column
        ))?;
        let counts: Vec<f64> = rows.iter().map(|r| r[0].parse::<f64>().unwrap()).collect();
        let sum: f64 = counts.iter().sum();
        let sum_sq: f64 = counts.iter().map(|c| c * c).sum();
        let total_links = (sum * sum - sum_sq) / 2.0;
        (opts.max_pairs / total_links).sqrt()
    } else {
        rows_needed_for_n_pairs(opts.max_pairs) / total_nodes
    };
    if proportion >= 1.0 {
        proportion = 1.0;
    }

    // Materialize the sample.
    let mut sp = CtePipeline::new().with_input(concat);
    let filter = sample_filter_sql(settings, proportion, opts.seed);
    sp.enqueue(
        format!("select *\nfrom {}\nwhere 1=1\n{filter}", concat.templated),
        "__dedupe__concat_sample",
    );
    let sample = session.materialize(&sp)?;

    let rhs_num_chunks = opts.num_chunks;
    // Collect estimates before mutating the model (borrow hygiene).
    let mut estimates: Vec<((usize, i32), ProbEstimate)> = Vec::new();

    for (ci, comparison) in settings.comparisons.iter().enumerate() {
        let mut acc = Accumulator::new(comparison);
        let use_probe = rhs_num_chunks > 1;
        let mut met = false;

        if use_probe {
            let probe_chunks = rhs_num_chunks * 10;
            let counts = run_chunk(session, settings, comparison, &sample, 1, probe_chunks)?;
            acc.update(&counts);
            if let Some(min) = opts.min_count_per_level {
                met = acc.all_meet(min);
            }
        }

        if !met {
            if use_probe {
                acc = Accumulator::new(comparison);
            }
            for chunk_num in 1..=rhs_num_chunks {
                let counts = run_chunk(
                    session,
                    settings,
                    comparison,
                    &sample,
                    chunk_num,
                    rhs_num_chunks,
                )?;
                acc.update(&counts);
                if let Some(min) = opts.min_count_per_level
                    && acc.all_meet(min)
                {
                    break;
                }
            }
        }

        // Normalize: u_i = count_i / sum(count over non-null).
        let total: f64 = acc.counts.values().sum();
        for (gamma, count) in &acc.counts {
            let estimate = if *count == 0.0 {
                ProbEstimate::NotObserved
            } else {
                ProbEstimate::Value(count / total)
            };
            estimates.push(((ci, *gamma), estimate));
        }
    }

    session.drop_table(&sample)?;

    for (key, estimate) in estimates {
        model.add_u_estimate(key, estimate, EstimateSource::USampling);
    }
    model.populate_from_trained_values();
    Ok(())
}
