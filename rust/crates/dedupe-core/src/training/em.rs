//! Expectation-maximisation training, the without-term-frequencies path.
//!
//! One session trains the comparisons NOT consumed by the training blocking
//! rule, starting from a blocking-adjusted prior. Each iteration scores the
//! compressed agreement-pattern table (no TF adjustment) with the current
//! parameters, recomputes m (and the working prior) from match-probability
//! weighted counts, and stops when the largest parameter change drops below
//! `em_convergence`. Trained m values across sessions combine by median.

use std::collections::BTreeMap;

use crate::comparison::Comparison;
use crate::error::{DedupeError, Result};
use crate::model::{EstimateSource, ProbEstimate, TrainedModel};
use crate::scoring;
use crate::session::{Session, TableHandle};
use crate::settings::Settings;
use crate::sql::literals::pyrepr;
use crate::sql::pipeline::CtePipeline;
use crate::stages::vectors::gamma_case_sql;

/// A single comparison level's trained parameters within an EM iteration.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LevelParams {
    pub gamma: i32,
    pub m: f64,
    pub u: f64,
}

/// One EM iteration's full parameter state (per active comparison).
#[derive(Debug, Clone)]
pub struct Iteration {
    pub lambda: f64,
    /// Keyed by comparison output column name -> levels (non-null).
    pub params: BTreeMap<String, Vec<LevelParams>>,
}

#[derive(Debug, Clone)]
pub struct EmSessionResult {
    pub blocking_rule: String,
    pub deactivated: Vec<String>,
    pub blocking_adjusted_lambda: f64,
    /// Per-iteration state, index 0 = initial, then one per EM step.
    pub iterations: Vec<Iteration>,
    pub converged: bool,
}

/// Working per-level state during a session (resolved concrete values plus the
/// "not observed" flag so the sentinel propagates to trained output).
#[derive(Debug, Clone)]
struct WorkingLevel {
    gamma: i32,
    is_null: bool,
    m: ProbValue,
    u: ProbValue,
    fix_m: bool,
    fix_u: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum ProbValue {
    Value(f64),
    NotObserved,
}

impl ProbValue {
    fn get(&self) -> f64 {
        match self {
            ProbValue::Value(v) => *v,
            ProbValue::NotObserved => crate::comparison::LEVEL_NOT_OBSERVED_VALUE,
        }
    }
}

#[derive(Debug, Clone)]
struct WorkingComparison {
    output_column_name: String,
    levels: Vec<WorkingLevel>,
    gamma_prefix: String,
    mw_prefix: String,
}

impl WorkingComparison {
    fn gamma_column_name(&self) -> String {
        format!(
            "{}{}",
            self.gamma_prefix,
            self.output_column_name.replace(' ', "_")
        )
    }
    fn mw_column_name(&self) -> String {
        format!(
            "{}{}",
            self.mw_prefix,
            self.output_column_name.replace(' ', "_")
        )
    }
}

pub struct EmOptions {
    pub fix_m: bool,
    pub fix_u: bool,
    pub fix_lambda: bool,
}

impl Default for EmOptions {
    fn default() -> Self {
        // Default: EM trains m only.
        EmOptions {
            fix_m: false,
            fix_u: true,
            fix_lambda: false,
        }
    }
}

/// Columns referenced by the training blocking rule (base names).
fn blocking_rule_columns(sql: &str) -> Result<Vec<String>> {
    let cols = crate::sql::columns::columns_used(sql, "em blocking rule")?;
    let mut out = Vec::new();
    for c in cols {
        if !out.contains(&c.column) {
            out.push(c.column);
        }
    }
    Ok(out)
}

/// Blocking-adjusted starting prior: multiply the prior Bayes factor by the
/// Bayes factor of each exact-match level whose columns are a subset of the
/// blocking rule's columns, preferring levels covering more columns.
fn blocking_adjusted_lambda(settings: &Settings, prior: f64, rule_columns: &[String]) -> f64 {
    let mut remaining: Vec<String> = rule_columns.to_vec();
    let mut bf = prior / (1.0 - prior);

    // All exact-match levels, sorted by descending column count.
    let mut exact_levels: Vec<(&Comparison, &crate::comparison::ComparisonLevel)> = Vec::new();
    for c in &settings.comparisons {
        for l in &c.levels {
            if l.exact_match_columns.is_some() {
                exact_levels.push((c, l));
            }
        }
    }
    exact_levels
        .sort_by_key(|(_, l)| std::cmp::Reverse(l.exact_match_columns.as_ref().unwrap().len()));

    for (c, level) in exact_levels {
        let cols = level.exact_match_columns.as_ref().unwrap();
        if cols.iter().all(|c| remaining.contains(c)) {
            remaining.retain(|r| !cols.contains(r));
            let (m, u) = scoring::level_m_u(c, level).unwrap();
            bf *= m / u;
        }
    }
    bf / (1.0 + bf)
}

fn build_working(settings: &Settings, deactivated: &[String]) -> Vec<WorkingComparison> {
    settings
        .comparisons
        .iter()
        .filter(|c| !deactivated.contains(&c.output_column_name))
        .map(|c| WorkingComparison {
            output_column_name: c.output_column_name.clone(),
            gamma_prefix: settings.gamma_prefix.clone(),
            mw_prefix: settings.mw_prefix.clone(),
            levels: c
                .levels
                .iter()
                .map(|l| WorkingLevel {
                    gamma: l.gamma,
                    is_null: l.is_null_level,
                    m: ProbValue::Value(l.m.resolve(l.default_m)),
                    u: ProbValue::Value(l.u.resolve(l.default_u)),
                    fix_m: l.fix_m,
                    fix_u: l.fix_u,
                })
                .collect(),
        })
        .collect()
}

/// Comparison-vectors pipeline for the training rule (no TF-free blocking,
/// then TF-joined comparison vectors). Only active comparisons' gammas are
/// produced. Returns the materialized comparison-vectors handle.
fn materialize_training_vectors(
    session: &Session,
    settings: &Settings,
    concat: &TableHandle,
    tf_tables: &[TableHandle],
    active: &[&Comparison],
    blocking_rule: &str,
) -> Result<TableHandle> {
    use crate::stages::block::BLOCKED_ID_PAIRS;
    use crate::stages::tf::{self, DF_CONCAT_WITH_TF};
    use crate::stages::uid_expr_for;
    use crate::stages::vectors::BLOCKED_WITH_COLS;

    let uid = uid_expr_for(settings);
    let rule = crate::blocking::BlockingRule {
        index: 0,
        sql: blocking_rule.to_string(),
        arrays_to_explode: Vec::new(),
    };

    // Blocked pairs from the single training rule, on __splink__df_concat.
    let mut bp = CtePipeline::new().with_input(concat);
    bp.enqueue(
        crate::stages::block::blocked_id_pairs_sql(
            std::slice::from_ref(&rule),
            &uid,
            settings.link_type,
            &settings.source_dataset_column,
            concat.templated.as_str(),
            &[],
            None,
        ),
        BLOCKED_ID_PAIRS,
    );
    let blocked = session.materialize(&bp)?;

    // Active comparisons' TF columns (for the concat_with_tf join + l/r cols).
    let mut tf_cols: Vec<String> = Vec::new();
    for c in active {
        for col in c.tf_input_columns() {
            if !tf_cols.contains(&col) {
                tf_cols.push(col);
            }
        }
    }

    let mut p = CtePipeline::new().with_input(concat).with_input(&blocked);
    for t in tf_tables {
        p.add_input(t);
    }
    p.enqueue(
        tf::concat_with_tf_sql(&tf_cols, &settings.tf_prefix),
        DF_CONCAT_WITH_TF,
    );

    // blocked_with_cols for active comparisons only.
    let mut sel: Vec<String> = Vec::new();
    let push_lr = |sel: &mut Vec<String>, col: &str| {
        sel.push(format!("\"l\".\"{col}\" AS \"{col}_l\""));
        sel.push(format!("\"r\".\"{col}\" AS \"{col}_r\""));
    };
    if settings.source_dataset_required() {
        push_lr(&mut sel, &settings.source_dataset_column);
    }
    push_lr(&mut sel, &settings.unique_id_column);
    for c in active {
        for col in &c.input_columns {
            push_lr(&mut sel, col);
        }
        for col in c.tf_input_columns() {
            push_lr(&mut sel, &format!("{}{col}", settings.tf_prefix));
        }
    }
    let mut dedup: Vec<String> = Vec::new();
    for s in sel {
        if !dedup.contains(&s) {
            dedup.push(s);
        }
    }
    let bwc = format!(
        "select\n    {},\n    b.match_key\nfrom {BLOCKED_ID_PAIRS} as b\ninner join {DF_CONCAT_WITH_TF} as l\non {} = b.join_key_l\ninner join {DF_CONCAT_WITH_TF} as r\non {} = b.join_key_r",
        dedup.join(",\n    "),
        uid.render("l"),
        uid.render("r"),
    );
    p.enqueue(bwc, BLOCKED_WITH_COLS);

    // comparison vectors: uid + gamma CASE per active comparison + match_key.
    let mut cv: Vec<String> = Vec::new();
    cv.push(format!("\"{}_l\"", settings.unique_id_column));
    cv.push(format!("\"{}_r\"", settings.unique_id_column));
    for c in active {
        cv.push(gamma_case_sql(c));
    }
    cv.push("match_key".to_string());
    p.enqueue(
        format!(
            "select\n    {}\nfrom {BLOCKED_WITH_COLS}",
            cv.join(",\n    ")
        ),
        "__splink__df_comparison_vectors",
    );

    let result = session.materialize(&p)?;
    session.drop_table(&blocked)?;
    Ok(result)
}

/// Compress comparison vectors to agreement-pattern counts over active gammas.
fn materialize_agreement_patterns(
    session: &Session,
    working: &[WorkingComparison],
    vectors: &TableHandle,
) -> Result<TableHandle> {
    let gammas: Vec<String> = working
        .iter()
        .map(WorkingComparison::gamma_column_name)
        .collect();
    let group = gammas.join(", ");
    let sql = format!(
        "select {group}, count(*) as agreement_pattern_count\nfrom {}\ngroup by {group}",
        vectors.templated
    );
    let mut p = CtePipeline::new().with_input(vectors);
    p.enqueue(sql, "__splink__agreement_pattern_counts");
    session.materialize(&p)
}

/// The `mw_` CASE for one working comparison at the current parameters
/// (no TF adjustment — EM scores without TF in this path).
fn mw_case(c: &WorkingComparison) -> String {
    let gamma = c.gamma_column_name();
    let mut arms = Vec::new();
    for level in &c.levels {
        if level.is_null {
            arms.push(format!("    WHEN {gamma} = -1 THEN cast(0.0 as float8)"));
        } else {
            let mw = scoring::match_weight(level.m.get(), level.u.get());
            arms.push(format!(
                "    WHEN {gamma} = {} THEN cast({} as float8)",
                level.gamma,
                pyrepr(mw)
            ));
        }
    }
    format!("CASE\n{}\nEND as {}", arms.join("\n"), c.mw_column_name())
}

/// One E-step + count step over agreement patterns, returning per-comparison
/// (gamma -> (m_count, u_count)) plus the lambda numerator/denominator.
#[allow(clippy::type_complexity)]
fn run_counts(
    session: &Session,
    working: &[WorkingComparison],
    lambda: f64,
    patterns: &TableHandle,
) -> Result<(BTreeMap<String, BTreeMap<i32, (f64, f64)>>, f64)> {
    // match weight expression = prior log2-odds + sum of active mw columns.
    let mut terms = vec![format!(
        "cast({} as float8)",
        pyrepr(scoring::prior_log2_odds(lambda))
    )];
    let mut mw_cols = Vec::new();
    for c in working {
        mw_cols.push(mw_case(c));
        terms.push(c.mw_column_name());
    }
    let mw_expr = terms.join(" + ");

    let mwp_sql = format!(
        "select\n    {},\n    agreement_pattern_count\nfrom {}",
        mw_cols
            .iter()
            .zip(working.iter())
            .map(|(case, c)| format!("{},\n    {case}", c.gamma_column_name()))
            .collect::<Vec<_>>()
            .join(",\n    "),
        patterns.templated
    );

    let predict_sql = format!(
        "select\n    CASE WHEN ({mw_expr}) >= 0 THEN 1.0 / (1.0 + POWER(cast(2 as float8), -({mw_expr}))) ELSE POWER(cast(2 as float8), ({mw_expr})) / (1.0 + POWER(cast(2 as float8), ({mw_expr}))) END as match_probability,\n    {},\n    agreement_pattern_count\nfrom __splink__df_match_weight_parts",
        working
            .iter()
            .map(WorkingComparison::gamma_column_name)
            .collect::<Vec<_>>()
            .join(",\n    ")
    );

    let mut count_blocks: Vec<String> = working
        .iter()
        .map(|c| {
            let g = c.gamma_column_name();
            format!(
                "select {g} as comparison_vector_value, sum(match_probability * agreement_pattern_count) as m_count, sum((1-match_probability) * agreement_pattern_count) as u_count, '{}' as output_column_name from __splink__df_predict group by {g}",
                c.output_column_name
            )
        })
        .collect();
    count_blocks.push(
        "select 0 as comparison_vector_value, sum(match_probability * agreement_pattern_count) / sum(agreement_pattern_count) as m_count, sum((1-match_probability) * agreement_pattern_count) / sum(agreement_pattern_count) as u_count, '_probability_two_random_records_match' as output_column_name from __splink__df_predict".to_string(),
    );
    let counts_sql = count_blocks.join("\n\nUNION ALL\n\n");

    let mut p = CtePipeline::new().with_input(patterns);
    p.enqueue(mwp_sql, "__splink__df_match_weight_parts");
    p.enqueue(predict_sql, "__splink__df_predict");
    p.enqueue(counts_sql, "__splink__m_u_counts");
    let counts = session.materialize(&p)?;

    let rows = session.query_strings(&format!(
        "select output_column_name, comparison_vector_value, m_count, u_count from {}",
        counts.physical
    ))?;
    session.drop_table(&counts)?;

    let mut per_comp: BTreeMap<String, BTreeMap<i32, (f64, f64)>> = BTreeMap::new();
    let mut lambda_num = f64::NAN;
    for row in rows {
        let name = &row[0];
        let cvv: i32 = row[1].parse().unwrap();
        let m_count: f64 = row[2].parse().unwrap();
        let u_count: f64 = row[3].parse().unwrap();
        if name == "_probability_two_random_records_match" {
            lambda_num = m_count;
        } else {
            per_comp
                .entry(name.clone())
                .or_default()
                .insert(cvv, (m_count, u_count));
        }
    }
    Ok((per_comp, lambda_num))
}

/// Run one EM session and append trained m (and u if not fixed) to the model.
pub fn run_em_session(
    session: &Session,
    model: &mut TrainedModel,
    concat: &TableHandle,
    tf_tables: &[TableHandle],
    blocking_rule: &str,
    opts: EmOptions,
) -> Result<EmSessionResult> {
    let rule_columns = blocking_rule_columns(blocking_rule)?;

    // Deactivate comparisons whose case-statement columns intersect the rule.
    let deactivated: Vec<String> = model
        .settings
        .comparisons
        .iter()
        .filter(|c| c.input_columns.iter().any(|col| rule_columns.contains(col)))
        .map(|c| c.output_column_name.clone())
        .collect();

    let active_indices: Vec<usize> = (0..model.settings.comparisons.len())
        .filter(|i| !deactivated.contains(&model.settings.comparisons[*i].output_column_name))
        .collect();
    let active: Vec<&Comparison> = active_indices
        .iter()
        .map(|i| &model.settings.comparisons[*i])
        .collect();

    let adjusted_lambda =
        blocking_adjusted_lambda(&model.settings, model.settings.prior, &rule_columns);

    let vectors = materialize_training_vectors(
        session,
        &model.settings,
        concat,
        tf_tables,
        &active,
        blocking_rule,
    )?;
    if session.count(&vectors.physical)? == 0 {
        return Err(DedupeError::Other(format!(
            "training rule `{blocking_rule}` produced no record pairs"
        )));
    }

    let mut working = build_working(&model.settings, &deactivated);
    let patterns = materialize_agreement_patterns(session, &working, &vectors)?;

    let max_iterations = model.settings.max_iterations;
    let em_convergence = model.settings.em_convergence;
    let mut lambda = adjusted_lambda;
    let mut iterations = vec![snapshot(&working, lambda)];
    let mut converged = false;

    for _ in 1..=max_iterations {
        let prev = iterations.last().unwrap().clone();
        let (counts, lambda_num) = run_counts(session, &working, lambda, &patterns)?;

        // M-step. Normalize m and u per comparison over non-null levels.
        for c in &mut working {
            let Some(level_counts) = counts.get(&c.output_column_name) else {
                continue;
            };
            // Normalization denominators exclude the null level (gamma = -1),
            // i.e. `where comparison_vector_value != -1`.
            let m_total: f64 = level_counts
                .iter()
                .filter(|(g, _)| **g != -1)
                .map(|(_, (m, _))| m)
                .sum();
            let u_total: f64 = level_counts
                .iter()
                .filter(|(g, _)| **g != -1)
                .map(|(_, (_, u))| u)
                .sum();
            for level in &mut c.levels {
                if level.is_null {
                    continue;
                }
                if let Some((m_count, u_count)) = level_counts.get(&level.gamma) {
                    if !opts.fix_m && !level.fix_m {
                        level.m = ProbValue::Value(m_count / m_total);
                    }
                    if !opts.fix_u && !level.fix_u {
                        level.u = ProbValue::Value(u_count / u_total);
                    }
                } else {
                    if !opts.fix_m && !level.fix_m {
                        level.m = ProbValue::NotObserved;
                    }
                    if !opts.fix_u && !level.fix_u {
                        level.u = ProbValue::NotObserved;
                    }
                }
            }
        }
        if !opts.fix_lambda {
            lambda = lambda_num;
        }

        let snap = snapshot(&working, lambda);
        let max_change = max_abs_change(&prev, &snap);
        iterations.push(snap);
        if max_change < em_convergence {
            converged = true;
            break;
        }
    }

    session.drop_table(&patterns)?;
    session.drop_table(&vectors)?;

    // Collect trained values, then apply (avoids borrowing model mutably while
    // iterating its comparisons).
    let source = EstimateSource::Em {
        blocking_rule: blocking_rule.to_string(),
    };
    let mut m_estimates: Vec<((usize, i32), ProbEstimate)> = Vec::new();
    let mut u_estimates: Vec<((usize, i32), ProbEstimate)> = Vec::new();
    for (ci, c) in model.settings.comparisons.iter().enumerate() {
        if deactivated.contains(&c.output_column_name) {
            continue;
        }
        let wc = working
            .iter()
            .find(|w| w.output_column_name == c.output_column_name)
            .unwrap();
        for wl in &wc.levels {
            if wl.is_null {
                continue;
            }
            let key = (ci, wl.gamma);
            let to_est = |v: ProbValue| match v {
                ProbValue::Value(x) => ProbEstimate::Value(x),
                ProbValue::NotObserved => ProbEstimate::NotObserved,
            };
            if !opts.fix_m {
                m_estimates.push((key, to_est(wl.m)));
            }
            if !opts.fix_u {
                u_estimates.push((key, to_est(wl.u)));
            }
        }
    }
    for (key, est) in m_estimates {
        model.add_m_estimate(key, est, source.clone());
    }
    for (key, est) in u_estimates {
        model.add_u_estimate(key, est, source.clone());
    }
    model.populate_from_trained_values();

    Ok(EmSessionResult {
        blocking_rule: blocking_rule.to_string(),
        deactivated,
        blocking_adjusted_lambda: adjusted_lambda,
        iterations,
        converged,
    })
}

fn snapshot(working: &[WorkingComparison], lambda: f64) -> Iteration {
    let mut params = BTreeMap::new();
    for c in working {
        let levels: Vec<LevelParams> = c
            .levels
            .iter()
            .filter(|l| !l.is_null)
            .map(|l| LevelParams {
                gamma: l.gamma,
                m: l.m.get(),
                u: l.u.get(),
            })
            .collect();
        params.insert(c.output_column_name.clone(), levels);
    }
    Iteration { lambda, params }
}

/// Convergence metric: max over non-null levels of max(|Δm|, |Δu|),
/// compared against |Δλ|.
fn max_abs_change(prev: &Iteration, this: &Iteration) -> f64 {
    let mut max_change = -0.1_f64;
    for (name, prev_levels) in &prev.params {
        let this_levels = &this.params[name];
        for (p, t) in prev_levels.iter().zip(this_levels) {
            let change = (t.m - p.m).abs().max((t.u - p.u).abs());
            if change > max_change {
                max_change = change;
            }
        }
    }
    let dlambda = (this.lambda - prev.lambda).abs();
    if dlambda > max_change {
        max_change = dlambda;
    }
    max_change
}
