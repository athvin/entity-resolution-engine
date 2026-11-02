//! `er profile` — profile a corpus stage and persist the result.
//!
//! The three subjects are the same corpus at three stages, and they run the
//! same aggregates: `standardized` is the canonical attributes the model
//! compares, `golden` is what resolution produced, and `raw` is the delivered
//! payload before any mapping. That last one is the only awkward case — a
//! delivered row is a JSON object, so its "columns" are keys rather than
//! columns, and it has to be flattened before it can be profiled like the
//! others. Doing it that way rather than writing a second JSON-shaped profiler
//! is what keeps one set of aggregates (and one golden) covering all three.
//!
//! Persistence is the staging-temp-plus-MERGE pattern `write_metric_snapshot`
//! established: build a temp table shaped like the target, call the generated
//! upsert, drop the temp. The MERGE keys on each table's natural key, so
//! re-profiling an unchanged lake snapshot rewrites rows in place instead of
//! appending a second copy — which is what makes the stored profile a cache.

use anyhow::{Context, Result, bail};
use dedupe_core::blocking_analysis::{CandidateKeyEstimate, rank_candidate_keys};
use dedupe_core::profile::{
    ColumnShape, ProfileOptions, ValueFrequency, column_shape, read_column_shapes, top_values,
};
use dedupe_core::session::{Session, TableHandle};
use er_profile::{Finding, findings_with};

use crate::create_ddl_table;

/// How many of each column's most frequent values to keep.
///
/// Ten, and bounded in SQL rather than after the fact: this list is stored on a
/// per-column row, so an unbounded one would put the corpus inside the profile.
const TOP_K: usize = 10;

/// The sentinel `source_system` for a subject that is not source-scoped.
///
/// A sentinel rather than NULL because `source_system` is part of
/// `profile_runs`' natural key, and a NULL key column never matches in a
/// MERGE's ON clause — every re-profile would insert a duplicate instead of
/// updating.
pub const NO_SOURCE: &str = "*";

/// Which stage of the corpus to profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Subject {
    /// Delivered rows, before mapping — per source.
    Raw,
    /// The standardized canonical attributes.
    Standardized,
    /// Post-resolution golden records.
    Golden,
}

impl Subject {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Subject::Raw => "raw",
            Subject::Standardized => "standardized",
            Subject::Golden => "golden",
        }
    }

    /// Parse a CLI value.
    ///
    /// # Errors
    /// Names the accepted values rather than echoing the bad one back alone.
    pub fn parse(raw: &str) -> Result<Self> {
        match raw {
            "raw" => Ok(Subject::Raw),
            "standardized" => Ok(Subject::Standardized),
            "golden" => Ok(Subject::Golden),
            other => bail!("unknown profile subject {other:?} (raw, standardized or golden)"),
        }
    }
}

/// What a profile produced, for the CLI to render and a caller to assert on.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ProfileReport {
    /// The persisted run's id — the handle a later read uses to fetch this
    /// profile's columns and findings back out of the lake.
    pub profile_run_id: String,
    pub subject: &'static str,
    pub source_system: String,
    pub snapshot: i64,
    pub tier: i64,
    pub row_count: i64,
    pub column_count: i64,
    pub sampled: bool,
    pub duration_ms: i64,
    pub columns: Vec<ColumnShapeOut>,
    /// Candidate blocking keys, cheapest first. Empty below tier 2.
    pub candidate_keys: Vec<CandidateKeyOut>,
    pub findings: Vec<FindingOut>,
}

/// One candidate blocking key's cost, flattened for JSON output.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CandidateKeyOut {
    pub key_columns: Vec<String>,
    pub expected_pairs: i64,
    pub largest_block_pairs: i64,
    pub distinct_values: i64,
    pub covered_rows: i64,
    pub entropy_bits: f64,
    pub pair_loss_from_nulls: f64,
}

/// A column's shape, flattened for JSON output.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ColumnShapeOut {
    pub column_name: String,
    pub fill_rate: Option<f64>,
    pub cardinality_ratio: Option<f64>,
    pub is_unique: Option<bool>,
    pub non_null_count: i64,
    pub blank_count: i64,
    pub distinct_approx: i64,
    pub distinct_exact: Option<i64>,
    pub entropy_bits: Option<f64>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct FindingOut {
    pub code: String,
    pub severity: &'static str,
    pub column_name: String,
    pub message: String,
}

/// Profile one corpus stage, record it, and return what was found.
///
/// `captured_at` is the run's logical clock rather than wall time, so a replayed
/// fixture produces the same row.
///
/// # Errors
/// Propagates the lake read and write. Refuses a subject whose relation is
/// absent, rather than recording an empty profile that would read as "this
/// corpus has no columns".
#[allow(clippy::too_many_arguments)]
pub fn run_profile(
    session: &Session,
    subject: Subject,
    source: Option<&str>,
    tier: i64,
    options: &ProfileOptions,
    account: bool,
    captured_at: &str,
) -> Result<ProfileReport> {
    let started = std::time::Instant::now();
    for relation in [
        "profile_runs",
        "column_profiles",
        "profile_findings",
        "candidate_keys",
    ] {
        create_ddl_table(session, relation)?;
    }
    let snapshot = session
        .latest_snapshot_version()
        .context("reading the lake's current snapshot")?;

    let (input, source_system) = materialize_subject(session, subject, source, account, snapshot)?;
    let columns = input.columns.clone();
    if columns.is_empty() {
        bail!("the {} corpus has no columns to profile", subject.as_str());
    }
    let corpus_digest = content_digest(session, &input, &columns)?;

    let shape_table = column_shape(session, &input, &columns, options)?;
    let shapes = read_column_shapes(session, &shape_table)?;
    session.drop_table(&shape_table)?;
    let tops = top_values(session, &input, &columns, TOP_K, options)?;

    let paired: Vec<(ColumnShape, Vec<ValueFrequency>)> = shapes
        .iter()
        .map(|s| {
            let mine = tops
                .iter()
                .filter(|v| v.column_name == s.column_name)
                .cloned()
                .collect();
            (s.clone(), mine)
        })
        .collect();

    // Tier 2: what each plausible column would cost as a blocking key. This is
    // the half of the profile aimed at the model rather than at the reader, and
    // it needs no trained model to answer — which is the point, because
    // blocking is chosen before anything is fit.
    let candidates = if tier >= TIER_BLOCKING {
        let (keys, considered) = candidate_columns(&shapes);
        if considered > keys.len() {
            eprintln!(
                "profile: measuring {} of {considered} candidate keys (cap {MAX_CANDIDATE_KEYS}, \
                 most selective first)",
                keys.len()
            );
        }
        rank_candidate_keys(session, &input, &keys)?
    } else {
        Vec::new()
    };
    let found = findings_with(&paired, &candidates);

    let row_count = shapes.first().map_or(0, |s| s.row_count);
    let duration_ms = i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX);

    let run = ProfileRunRow {
        subject: subject.as_str(),
        source_system: &source_system,
        snapshot,
        tier,
        row_count,
        column_count: i64::try_from(shapes.len()).unwrap_or(i64::MAX),
        sampled: options.sample.is_some(),
        sample_proportion: options.sample.as_ref().map(|s| s.proportion),
        sample_seed: options.sample.as_ref().and_then(|s| s.seed),
        duration_ms,
        corpus_digest: &corpus_digest,
        captured_at,
    };
    let profile_run_id = write_profile(session, &run, &shapes, &tops, &found, &candidates)?;
    // `input` is a snapshot-pinned temp VIEW, not a table, so it is left for the
    // session to reclaim — `drop_table` issues DROP TABLE and errors on it. No
    // caller of `register_lake_table_at_version` drops its handle, for the same
    // reason, and each call gets its own physical name so repeats never collide.

    Ok(ProfileReport {
        profile_run_id,
        subject: subject.as_str(),
        source_system,
        snapshot,
        tier,
        row_count,
        column_count: i64::try_from(shapes.len()).unwrap_or(i64::MAX),
        sampled: options.sample.is_some(),
        duration_ms,
        candidate_keys: candidates
            .iter()
            .map(|c| CandidateKeyOut {
                key_columns: c.key_columns.clone(),
                expected_pairs: c.expected_pairs,
                largest_block_pairs: c.largest_block_pairs,
                distinct_values: c.distinct_values,
                covered_rows: c.covered_rows,
                entropy_bits: c.entropy_bits,
                pair_loss_from_nulls: c.pair_loss_from_nulls,
            })
            .collect(),
        columns: shapes
            .iter()
            .map(|s| ColumnShapeOut {
                column_name: s.column_name.clone(),
                fill_rate: s.fill_rate(),
                cardinality_ratio: s.cardinality_ratio(),
                is_unique: s.is_unique(),
                non_null_count: s.non_null_count,
                blank_count: s.blank_count,
                distinct_approx: s.distinct_approx,
                distinct_exact: s.distinct_exact,
                entropy_bits: s.entropy_bits,
            })
            .collect(),
        findings: found
            .iter()
            .map(|f| FindingOut {
                code: f.code.0.to_string(),
                severity: f.severity.as_str(),
                column_name: f.column_name.clone(),
                message: f.message.clone(),
            })
            .collect(),
    })
}

/// The tier at which candidate blocking keys are measured.
pub const TIER_BLOCKING: i64 = 2;

/// How many candidate keys to measure, at most.
///
/// Each one is its own `GROUP BY`, so an unbounded fan-out over a wide
/// delivered file would turn a profile into a scan per column. Twenty-five is
/// well past the number of columns anyone blocks on and bounded enough to stay
/// a profile; the columns are taken most-selective-first, and the cap is
/// REPORTED rather than applied silently, because a truncated list that looks
/// complete is the shape that makes someone trust a key that was never measured.
const MAX_CANDIDATE_KEYS: usize = 25;

/// Columns worth measuring as a blocking key, most selective first.
///
/// Filtered from the tier-1 shape rather than measured blind: a constant column
/// is the cartesian product and a mostly-empty one cannot reach its pairs, and
/// both are already reported by their own findings. Spending a `GROUP BY` to
/// re-discover that would make the deep tier slower for no new information.
fn candidate_columns(shapes: &[ColumnShape]) -> (Vec<Vec<String>>, usize) {
    let mut ranked: Vec<(&ColumnShape, f64)> = shapes
        .iter()
        .filter(|s| {
            let populated = s.non_null_count - s.blank_count;
            // Half, matching P002's threshold: below it the column's own
            // finding already says a key on it discards most pairs.
            let usable = s.row_count > 0 && populated * 2 >= s.row_count;
            // More than one value, or the key is `1=1`.
            let varied = s.distinct_exact.unwrap_or(s.distinct_approx) > 1;
            usable && varied
        })
        .filter_map(|s| s.cardinality_ratio().map(|r| (s, r)))
        .collect();
    // Most selective first, name as the tie-break so the list is stable.
    ranked.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.column_name.cmp(&b.0.column_name))
    });
    let considered = ranked.len();
    let keys = ranked
        .into_iter()
        .take(MAX_CANDIDATE_KEYS)
        .map(|(s, _)| vec![s.column_name.clone()])
        .collect();
    (keys, considered)
}

/// A content digest of the rows about to be profiled — the cache key.
///
/// One hash per row over all its columns, summed, folded together with the row
/// count. The sum makes it order-independent (so a compaction that rewrites
/// files in a different order does not look like new data), while hashing the
/// whole row at once keeps it position-sensitive (two columns swapping values
/// is a change). One scan, no sort.
///
/// This is the key rather than the lake snapshot because the snapshot counter is
/// per-catalog and advances on ANY write, including the profile's own: keying on
/// it meant every profile invalidated itself and appended a duplicate instead of
/// updating. A collision here would serve a stale profile, which is why the row
/// count is folded in alongside the hash sum rather than relying on the sum
/// alone.
fn content_digest(session: &Session, input: &TableHandle, columns: &[String]) -> Result<String> {
    let row = columns
        .iter()
        .map(|c| {
            let ident = c.replace('"', "\"\"");
            // chr(0) stands in for NULL so that a NULL and an empty string are
            // distinguishable; concat_ws would otherwise skip the NULL and make
            // the two identical.
            format!("coalesce(cast(\"{ident}\" as varchar), chr(0))")
        })
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "select md5(cast(count(*) as varchar) || ':' || \
                    cast(coalesce(sum(hash(concat_ws(chr(31), {row}))), 0) as varchar)) \
         from {}",
        input.physical
    );
    let rows = session.query_scratch_strings(&sql)?;
    rows.first()
        .and_then(|r| r.first())
        .filter(|d| !d.is_empty())
        .cloned()
        .context("the corpus digest came back empty")
}

/// Register the subject's relation as a snapshot-pinned handle, flattening the
/// delivered payload first for the `raw` subject.
fn materialize_subject(
    session: &Session,
    subject: Subject,
    source: Option<&str>,
    account: bool,
    snapshot: i64,
) -> Result<(TableHandle, String)> {
    match subject {
        Subject::Standardized => {
            let relation = if account {
                "acct_int_std_records"
            } else {
                "int_std_records"
            };
            let handle = session
                .register_lake_table_at_version(relation, "__profile_std", snapshot)
                .with_context(|| format!("opening {relation} at snapshot {snapshot}"))?;
            Ok((handle, NO_SOURCE.to_string()))
        }
        Subject::Golden => {
            let relation = if account {
                "acct_golden_records"
            } else {
                "golden_records"
            };
            let handle = session
                .register_lake_table_at_version(relation, "__profile_golden", snapshot)
                .with_context(|| format!("opening {relation} at snapshot {snapshot}"))?;
            Ok((handle, NO_SOURCE.to_string()))
        }
        Subject::Raw => {
            let source = source.context(
                "profiling the raw subject needs --source: delivered rows are profiled per \
                 source system, because two sources' payloads share no schema and a profile \
                 merging them would describe neither",
            )?;
            let handle = flatten_raw_payload(session, source, snapshot)?;
            Ok((handle, source.to_string()))
        }
    }
}

/// Flatten `raw_records.payload` into a column per delivered key.
///
/// The keys are discovered from the data rather than from config, which is the
/// point: a profile of the delivered file has to be able to show the columns
/// nobody mapped, and config by definition only names the ones somebody did.
/// Tombstones are excluded — a deleted row's payload is NULL and would be
/// counted as every column being empty.
fn flatten_raw_payload(session: &Session, source: &str, snapshot: i64) -> Result<TableHandle> {
    let raw = session
        .register_lake_table_at_version("raw_records", "__profile_raw_src", snapshot)
        .with_context(|| format!("opening raw_records at snapshot {snapshot}"))?;
    let lit = source.replace('\'', "''");

    let keys = session.query_scratch_strings(&format!(
        "select distinct key from (
             select unnest(json_keys(payload)) as key
             from {src}
             where source_system = '{lit}' and payload is not null and not is_deleted
         ) order by key",
        src = raw.physical
    ))?;
    if keys.is_empty() {
        bail!(
            "no delivered rows for source {source:?} at snapshot {snapshot}, so there is \
             nothing to profile"
        );
    }
    let projection = keys
        .iter()
        .map(|row| {
            let key = &row[0];
            let escaped = key.replace('\'', "''");
            let ident = key.replace('"', "\"\"");
            format!("json_extract_string(payload, '{escaped}') as \"{ident}\"")
        })
        .collect::<Vec<_>>()
        .join(",\n       ");

    session.execute_batch("drop view if exists __profile_raw")?;
    session.execute_batch(&format!(
        "create temp view __profile_raw as
         select {projection}
         from {src}
         where source_system = '{lit}' and payload is not null and not is_deleted",
        src = raw.physical
    ))?;
    // `raw` is a pinned temp view and the new view selects FROM it, so it has to
    // outlive this function anyway.
    let columns = keys.iter().map(|r| r[0].clone()).collect();
    Ok(TableHandle {
        templated: "__profile_raw".to_string(),
        physical: "__profile_raw".to_string(),
        columns,
    })
}

/// The scalar half of a `profile_runs` row.
struct ProfileRunRow<'a> {
    subject: &'static str,
    source_system: &'a str,
    snapshot: i64,
    tier: i64,
    row_count: i64,
    column_count: i64,
    sampled: bool,
    sample_proportion: Option<f64>,
    sample_seed: Option<i64>,
    duration_ms: i64,
    corpus_digest: &'a str,
    captured_at: &'a str,
}

/// Stage and MERGE the three tables, returning the profile run's id.
fn write_profile(
    session: &Session,
    run: &ProfileRunRow<'_>,
    shapes: &[ColumnShape],
    tops: &[ValueFrequency],
    found: &[Finding],
    candidates: &[CandidateKeyEstimate],
) -> Result<String> {
    let at = run.captured_at.replace('\'', "''");
    let subject = run.subject.replace('\'', "''");
    let source = run.source_system.replace('\'', "''");
    let digest = run.corpus_digest.replace('\'', "''");

    session.execute_batch("drop table if exists __profile_run_src")?;
    session.execute_batch(&format!(
        "create temp table __profile_run_src as select
             '{subject}' as subject,
             '{source}' as source_system,
             cast({snapshot} as bigint) as snapshot,
             cast({tier} as bigint) as tier,
             cast(null as varchar) as ingest_batch_id,
             '{digest}' as corpus_digest,
             cast({row_count} as bigint) as row_count,
             cast({column_count} as bigint) as column_count,
             cast(null as varchar) as config_hash,
             cast(null as varchar) as model_version,
             cast({sampled} as boolean) as sampled,
             {proportion} as sample_proportion,
             {seed} as sample_seed,
             cast({duration} as bigint) as duration_ms,
             cast('{at}' as timestamp) as captured_at",
        snapshot = run.snapshot,
        tier = run.tier,
        row_count = run.row_count,
        column_count = run.column_count,
        sampled = run.sampled,
        proportion = run
            .sample_proportion
            .map_or("cast(null as double)".to_string(), |p| format!(
                "cast({p} as double)"
            )),
        seed = run
            .sample_seed
            .map_or("cast(null as bigint)".to_string(), |s| format!(
                "cast({s} as bigint)"
            )),
        duration = run.duration_ms,
    ))?;
    er_entities::profile_run::create::upsert(session.connection())?;
    session.execute_batch("drop table __profile_run_src")?;

    // Read the id back rather than recomputing the sha256 in Rust: two
    // implementations of one derivation is one more than can be kept in step,
    // and the SQL is the authority because it is what wrote the row.
    let cached = er_entities::profile_run::read::cached_live(
        session.connection(),
        run.subject,
        run.source_system,
        run.corpus_digest,
        run.tier,
    )?;
    let profile_run_id = cached
        .first()
        .map(|r| r.id.clone())
        .context("the profile run was written but could not be read back")?;
    let id_lit = profile_run_id.replace('\'', "''");

    write_column_profiles(session, &id_lit, &at, shapes, tops)?;
    write_findings(session, &id_lit, &at, found)?;
    write_candidate_keys(session, &id_lit, &at, candidates)?;
    Ok(profile_run_id)
}

fn write_column_profiles(
    session: &Session,
    profile_run_id: &str,
    at: &str,
    shapes: &[ColumnShape],
    tops: &[ValueFrequency],
) -> Result<()> {
    if shapes.is_empty() {
        return Ok(());
    }
    let arms: Vec<String> = shapes
        .iter()
        .map(|s| {
            let mine: Vec<&ValueFrequency> = tops
                .iter()
                .filter(|v| v.column_name == s.column_name)
                .collect();
            let top_json = serde_json::to_string(
                &mine
                    .iter()
                    .map(|v| {
                        serde_json::json!({
                            "value": v.value, "count": v.count, "share": v.share, "rank": v.rank
                        })
                    })
                    .collect::<Vec<_>>(),
            )
            .unwrap_or_else(|_| "[]".to_string());
            format!(
                "select '{run}' as profile_run_id, '{name}' as column_name,
                        cast({row_count} as bigint) as row_count,
                        cast({total} as bigint) as total_row_count,
                        cast({non_null} as bigint) as non_null_count,
                        cast({blank} as bigint) as blank_count,
                        cast({d_approx} as bigint) as distinct_approx,
                        {d_exact} as distinct_exact,
                        {d_norm} as distinct_normalized,
                        {len_min} as len_min, {len_max} as len_max,
                        {len_avg} as len_avg, {len_p50} as len_p50, {len_p95} as len_p95,
                        {lex_min} as lex_min, {lex_max} as lex_max,
                        {num_min} as num_min, {num_max} as num_max,
                        {num_avg} as num_avg, {num_std} as num_std,
                        cast({c_bigint} as bigint) as cast_bigint,
                        cast({c_double} as bigint) as cast_double,
                        cast({c_date} as bigint) as cast_date,
                        cast({c_ts} as bigint) as cast_timestamp,
                        cast({c_bool} as bigint) as cast_boolean,
                        {entropy} as entropy_bits,
                        cast('{top}' as json) as top_values,
                        cast('{at}' as timestamp) as captured_at",
                run = profile_run_id,
                name = s.column_name.replace('\'', "''"),
                row_count = s.row_count,
                total = s.total_row_count,
                non_null = s.non_null_count,
                blank = s.blank_count,
                d_approx = s.distinct_approx,
                d_exact = opt_i64(s.distinct_exact),
                d_norm = opt_i64(s.distinct_normalized),
                len_min = opt_i64(s.len_min),
                len_max = opt_i64(s.len_max),
                len_avg = opt_f64(s.len_avg),
                len_p50 = opt_i64(s.len_p50),
                len_p95 = opt_i64(s.len_p95),
                lex_min = opt_str(s.lex_min.as_deref()),
                lex_max = opt_str(s.lex_max.as_deref()),
                num_min = opt_f64(s.num_min),
                num_max = opt_f64(s.num_max),
                num_avg = opt_f64(s.num_avg),
                num_std = opt_f64(s.num_std),
                c_bigint = s.cast_bigint,
                c_double = s.cast_double,
                c_date = s.cast_date,
                c_ts = s.cast_timestamp,
                c_bool = s.cast_boolean,
                entropy = opt_f64(s.entropy_bits),
                top = top_json.replace('\'', "''"),
            )
        })
        .collect();

    session.execute_batch("drop table if exists __column_profile_src")?;
    session.execute_batch(&format!(
        "create temp table __column_profile_src as {}",
        arms.join("\nUNION ALL\n")
    ))?;
    er_entities::column_profile::create::upsert(session.connection())?;
    session.execute_batch("drop table __column_profile_src")?;
    Ok(())
}

fn write_findings(
    session: &Session,
    profile_run_id: &str,
    at: &str,
    found: &[Finding],
) -> Result<()> {
    if found.is_empty() {
        return Ok(());
    }
    let arms: Vec<String> = found
        .iter()
        .map(|f| {
            let detail = serde_json::to_string(&f.detail).unwrap_or_else(|_| "null".to_string());
            format!(
                "select '{run}' as profile_run_id, '{code}' as code,
                        '{column}' as column_name, '{severity}' as severity,
                        '{message}' as message, cast('{detail}' as json) as detail,
                        cast('{at}' as timestamp) as captured_at",
                run = profile_run_id,
                code = f.code.0,
                column = f.column_name.replace('\'', "''"),
                severity = f.severity.as_str(),
                message = f.message.replace('\'', "''"),
                detail = detail.replace('\'', "''"),
            )
        })
        .collect();

    session.execute_batch("drop table if exists __profile_finding_src")?;
    session.execute_batch(&format!(
        "create temp table __profile_finding_src as {}",
        arms.join("\nUNION ALL\n")
    ))?;
    er_entities::profile_finding::create::upsert(session.connection())?;
    session.execute_batch("drop table __profile_finding_src")?;
    Ok(())
}

/// Stage and MERGE the candidate-key measurements.
///
/// `key_columns` is joined with `chr(31)` to match the entity's declared
/// encoding: the natural key needs a single comparable value, and DuckLake has
/// no list equality to MERGE on.
fn write_candidate_keys(
    session: &Session,
    profile_run_id: &str,
    at: &str,
    candidates: &[CandidateKeyEstimate],
) -> Result<()> {
    if candidates.is_empty() {
        return Ok(());
    }
    let arms: Vec<String> = candidates
        .iter()
        .map(|c| {
            let joined = c.key_columns.join("\u{1f}").replace('\'', "''");
            format!(
                "select '{run}' as profile_run_id, '{joined}' as key_columns,
                        cast({rows} as bigint) as row_count,
                        cast({covered} as bigint) as covered_rows,
                        cast({distinct_values} as bigint) as distinct_values,
                        cast({pairs} as bigint) as expected_pairs,
                        cast({lb_pairs} as bigint) as largest_block_pairs,
                        cast({lb_rows} as bigint) as largest_block_rows,
                        cast({entropy} as double) as entropy_bits,
                        cast({loss} as double) as pair_loss_from_nulls,
                        cast('{at}' as timestamp) as captured_at",
                run = profile_run_id,
                rows = c.rows,
                covered = c.covered_rows,
                distinct_values = c.distinct_values,
                pairs = c.expected_pairs,
                lb_pairs = c.largest_block_pairs,
                lb_rows = c.largest_block_rows,
                entropy = finite(c.entropy_bits),
                loss = finite(c.pair_loss_from_nulls),
            )
        })
        .collect();

    session.execute_batch("drop table if exists __candidate_key_src")?;
    session.execute_batch(&format!(
        "create temp table __candidate_key_src as {}",
        arms.join("\nUNION ALL\n")
    ))?;
    er_entities::candidate_key::create::upsert(session.connection())?;
    session.execute_batch("drop table __candidate_key_src")?;
    Ok(())
}

/// A float safe to interpolate: the columns are NOT NULL, so a NaN or infinity
/// — which `log2` can produce on a degenerate distribution — has to become a
/// number rather than a NULL the insert would refuse.
fn finite(v: f64) -> f64 {
    if v.is_finite() { v } else { 0.0 }
}

fn opt_i64(v: Option<i64>) -> String {
    v.map_or("cast(null as bigint)".to_string(), |x| {
        format!("cast({x} as bigint)")
    })
}

fn opt_f64(v: Option<f64>) -> String {
    v.filter(|x| x.is_finite())
        .map_or("cast(null as double)".to_string(), |x| {
            format!("cast({x} as double)")
        })
}

fn opt_str(v: Option<&str>) -> String {
    v.map_or("cast(null as varchar)".to_string(), |x| {
        format!("'{}'", x.replace('\'', "''"))
    })
}
