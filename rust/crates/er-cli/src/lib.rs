//! `er-cli` — orchestrate the ER pipeline stages over a real DuckLake lake (P2).
//!
//! The parity-gated stage crates (`er-ingest`, `er-standardize`, `er-match`,
//! `er-reconcile`, `er-golden`) each produce their relation as SQL/logic over a
//! dedupe-core `Session`. This crate wires them into the S4.0 run sequence
//! writing the ACTUAL lake relations (`lake.main.*`), so the whole pipeline runs
//! in Rust against DuckLake — the step that retires Python `src/er`.
//!
//! DuckLake runs locally: the catalog is a `sqlite:`/`.ducklake` file and
//! `data_path` a local Parquet directory (no Postgres/object-store needed).
//! Engine `__dedupe__*` intermediates stay in the native temp catalog;
//! dedupe-core enforces that.
//!
//! This is built up stage by stage; ingest + standardize land first.

use std::path::Path;

use anyhow::Result;
use dedupe_core::Session;
use dedupe_core::Settings;
use dedupe_core::predict::run_predict_with_tf;
use dedupe_core::session::SessionOptions;
use dedupe_core::session::ducklake::DuckLakeConfig;
use er_config::Config;
use er_ingest::landing::{land_base_delivery, land_full_refresh};
use std::collections::BTreeMap;

use er_ids::DeterministicV7Factory;
use er_match::{build_frozen_tf_tables, load_model};
use er_reconcile::assertions::{Assertion, AssertionKind};
use er_reconcile::cluster::label_propagate;
use er_reconcile::never_cut::{PathEdge, never_cut_fixpoint};
use er_reconcile::reconcile::reconcile_plan;
use er_standardize::{build_acct_int_std_records, build_int_std_records};
use std::collections::BTreeSet;

/// The lake relations this crate reads/writes, fully qualified.
pub const RAW_RECORDS: &str = "lake.main.raw_records";
pub const INT_STD_RECORDS: &str = "lake.main.int_std_records";
/// Account standardized records (`entity_type: account`), the Account analogue of
/// [`INT_STD_RECORDS`].
pub const ACCT_INT_STD_RECORDS: &str = "lake.main.acct_int_std_records";
pub const MATCH_SCORES: &str = "lake.main.match_scores";
/// What the last successful match pass scored, and under which model — the
/// ledger an incremental pass diffs the corpus against.
pub const SCORED_RECORDS: &str = "lake.main.scored_records";
pub const ENTITY_MEMBERSHIP: &str = "lake.main.entity_membership";
pub const ENTITIES: &str = "lake.main.entities";
pub const ENTITY_EVENTS: &str = "lake.main.entity_events";
pub const GOLDEN_LINEAGE: &str = "lake.main.golden_lineage";
pub const GOLDEN_RECORDS: &str = "lake.main.golden_records";
/// Account golden projections (`entity_type: account`), analogues of
/// [`GOLDEN_LINEAGE`]/[`GOLDEN_RECORDS`]. The scoring + entity tables
/// (match_scores, entity_membership, entities) are SHARED and entity-agnostic —
/// one lake holds one entity_type (the config carries exactly one), so there is
/// no `acct_match_scores`/`acct_entities`.
pub const ACCT_GOLDEN_LINEAGE: &str = "lake.main.acct_golden_lineage";
pub const ACCT_GOLDEN_RECORDS: &str = "lake.main.acct_golden_records";
/// Steward golden field overrides (mass-update); applied over golden_records.
pub const GOLDEN_OVERRIDES: &str = "lake.main.golden_overrides";
/// Steward record overlays (protect/suppress), keyed `(record_key, kind)`.
pub const RECORD_OVERLAYS: &str = "lake.main.record_overlays";
pub const RUNS: &str = "lake.main.runs";
pub const REVIEW_QUEUE: &str = "lake.main.review_queue";
pub const ASSERTIONS: &str = "lake.main.assertions";
pub const INGEST_BATCHES: &str = "lake.main.ingest_batches";
/// The nickname lexicon `int_std_records` reads — a temp relation (read
/// cross-catalog while the lake write runs), seeded per session.
pub const LEXICON: &str = "nickname_variants";

/// Open a session with a local DuckLake catalog attached as `lake`. `catalog` is
/// the ATTACH metadata (a `.ducklake`/`sqlite:` path); `data_path` is where
/// Parquet data is written.
///
/// # Errors
/// Propagates the dedupe-core session open / DuckLake attach errors.
pub fn open_lake(catalog: &str, data_path: &str) -> Result<Session> {
    let mut session = Session::open_in_memory(SessionOptions::deterministic())?;
    session.attach_ducklake(&DuckLakeConfig {
        metadata: catalog.to_string(),
        data_path: Some(data_path.to_string()),
        ..DuckLakeConfig::default()
    })?;
    Ok(session)
}

/// Seed the nickname lexicon (variant_a, variant_b, active) as a temp relation.
///
/// # Errors
/// Propagates the session SQL error.
pub fn seed_lexicon(session: &Session, rows: &[(String, String)]) -> Result<()> {
    // Re-seedable. A session that runs the pipeline twice — which is what an
    // incremental batch after a base run IS — otherwise died here on "table
    // nickname_variants already exists", because this used a bare CREATE. The
    // lexicon is a session-local projection of the caller's rows, so replacing
    // it is a no-op in content whenever the rows are the same.
    session.execute_batch(&format!("drop table if exists {LEXICON}"))?;
    if rows.is_empty() {
        // Empty VALUES is a syntax error; an entity type with no nickname lexicon
        // (e.g. accounts) seeds an empty typed table instead. The non-empty path
        // below is unchanged, so the person corpus is byte-identical.
        session.execute_batch(&format!(
            "create table {LEXICON} (variant_a varchar, variant_b varchar, active boolean)"
        ))?;
        return Ok(());
    }
    let values: Vec<String> = rows
        .iter()
        .map(|(a, b)| {
            format!(
                "('{}', '{}', true)",
                a.replace('\'', "''"),
                b.replace('\'', "''")
            )
        })
        .collect();
    session.execute_batch(&format!(
        "create table {LEXICON} as select * from (values {}) as t(variant_a, variant_b, active)",
        values.join(", ")
    ))?;
    Ok(())
}

/// S4.1 ingest: land one source's delivery into `lake.main.raw_records` (created
/// on first call). `full_refresh` tombstones this source's omitted keys.
///
/// # Errors
/// Propagates the raw_records DDL / landing errors.
pub fn run_ingest(
    session: &Session,
    config: &Config,
    source: &str,
    csv_text: &str,
    ingest_batch_id: &str,
    ingested_at: &str,
    full_refresh: bool,
) -> Result<usize> {
    // The S5 registry DDL (idempotent), so repeated deliveries append to one
    // relation and a fresh lake gets the declared NOT NULLs.
    create_ddl_table(session, "raw_records")?;
    let spec = config
        .sources
        .get(source)
        .ok_or_else(|| anyhow::anyhow!("unknown source {source:?}"))?;
    let land = if full_refresh {
        land_full_refresh
    } else {
        land_base_delivery
    };
    land(
        session,
        source,
        spec,
        csv_text,
        ingest_batch_id,
        ingested_at,
    )
}

/// S4.2 standardize: (re)build `lake.main.int_std_records` from raw_records.
///
/// # Errors
/// Propagates the standardization SQL error.
pub fn run_standardize(session: &Session, config: &Config, run_at: &str) -> Result<()> {
    if config.entity_type.is_person() {
        session.execute_batch(&format!("drop table if exists {INT_STD_RECORDS}"))?;
        build_int_std_records(
            session,
            config,
            RAW_RECORDS,
            LEXICON,
            INT_STD_RECORDS,
            run_at,
        )
    } else {
        session.execute_batch(&format!("drop table if exists {ACCT_INT_STD_RECORDS}"))?;
        build_acct_int_std_records(session, config, RAW_RECORDS, ACCT_INT_STD_RECORDS, run_at)
    }
}

/// The committed model a `full` match scores against (S4.3.2): the frozen match
/// settings + the frozen `tf_lookup` snapshot keyed by `(model_version,
/// tf_snapshot_id)`.
pub struct MatchModel<'a> {
    pub settings_json: &'a str,
    pub tf_csv_path: &'a Path,
    pub model_version: &'a str,
    pub tf_snapshot_id: &'a str,
}

/// The `evidence` JSON expression (S4.3.5): `gamma_<col>` + `mw_<col>` for every
/// comparison, `mw_tf_adj_<col>` where the model carries the TF adjustment, and
/// the whole-pair `match_weight`. Valid wherever the retained prediction columns
/// are in scope (requires `retain_intermediate`, which `run_match` sets).
fn evidence_json(settings: &Settings) -> String {
    let mut parts: Vec<String> = Vec::new();
    for c in &settings.comparisons {
        let g = c.gamma_column_name();
        let mw = c.mw_column_name();
        parts.push(format!("'{g}', {g}"));
        parts.push(format!("'{mw}', {mw}"));
        if c.has_tf_adjustments() {
            let adj = c.mw_tf_adj_column_name();
            parts.push(format!("'{adj}', {adj}"));
        }
    }
    parts.push("'match_weight', match_weight".to_string());
    format!("json_object({})", parts.join(", "))
}

/// One candidate entity for an unsaved record.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ResolveCandidate {
    /// The record the payload matched.
    pub record_key: String,
    /// The entity that record currently belongs to, when it belongs to one. A
    /// record that has never been through reconcile has no entity yet, and
    /// saying `null` is more honest than inventing one.
    pub entity_id: Option<String>,
    pub match_probability: f64,
    /// Which band the score falls in under the org's thresholds:
    /// `auto_merge` / `review` / `reject`.
    pub band: String,
    /// The per-comparison evidence behind the score — the same waterfall the
    /// review queue shows, so a caller can explain the decision it just made.
    pub evidence: serde_json::Value,
}

/// What resolving an unsaved record returned.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ResolveResult {
    pub candidates: Vec<ResolveCandidate>,
    /// The best candidate's band, or `reject` when there are none — the single
    /// value a form or an ingestion pipeline actually branches on.
    pub decision: String,
    /// The lake snapshot the corpus was read at, so a caller can tell whether it
    /// resolved against fresh data.
    pub snapshot: i64,
}

/// Score an UNSAVED record against the resolved corpus and return its candidate
/// entities, without writing anything.
///
/// This is the online "resolve this record" capability: a form, an ingestion
/// pipeline or a middleware layer posts a record and learns, before saving it,
/// which entity it belongs to and how confident that is. It is how the engine
/// embeds in someone else's system rather than owning the data.
///
/// The payload goes through **the same standardization the corpus went
/// through** — the same normalizers, the same nickname lexicon, the same
/// declared source column mapping — because a score is only meaningful between
/// comparably prepared values. That is why this reuses
/// `build_int_std_records` against a temp table instead of normalizing the
/// payload some cheaper way.
///
/// Nothing is committed: the payload lands in the session's own catalog (never
/// `lake.main`), and the only lake access is a read. A caller that wants the
/// record saved sends it to the import path afterwards.
///
/// # Errors
/// Returns an error when the source is not declared in the config, when the
/// model will not load, or when the lake read fails. A corpus that has never
/// been standardized yields no candidates rather than an error — a first record
/// legitimately matches nothing.
pub fn resolve_record(
    session: &Session,
    config: &Config,
    model: &MatchModel,
    source: &str,
    payload: &serde_json::Value,
    lexicon: &[(String, String)],
    limit: usize,
) -> Result<ResolveResult> {
    if !config.sources.contains_key(source) {
        let known: Vec<&str> = config.sources.keys().map(String::as_str).collect();
        anyhow::bail!(
            "source {source:?} is not in the active config (known: {})",
            known.join(", ")
        );
    }
    let mut settings = load_model(model.settings_json)?;
    settings.retain_intermediate = true;

    // The corpus. A lake that has never standardized has nothing to match
    // against, which is a legitimate answer rather than a failure.
    let corpus = match session.register_lake_table("int_std_records", "int_std_records") {
        Ok(handle) => handle,
        Err(e) if is_missing_table(&e.to_string()) => {
            return Ok(ResolveResult {
                candidates: Vec::new(),
                decision: "reject".to_string(),
                snapshot: 0,
            });
        }
        Err(e) => return Err(e.into()),
    };

    // The payload, standardized exactly as an ingested record would be. The
    // synthetic ids are deliberately recognisable: if one ever shows up in the
    // lake, something wrote what it should not have.
    // Re-seed rather than seed: this session may already have run a pipeline
    // (the server reuses one per request, and a test may resolve after a full
    // run), and `seed_lexicon` creates the table outright. Re-seeding with the
    // same pairs is a no-op in content, and the lexicon has to be present or
    // standardization cannot expand nicknames — which would make the probe
    // incomparable to the corpus it is being scored against.
    session.execute_batch(&format!("drop table if exists {LEXICON}"))?;
    seed_lexicon(session, lexicon)?;
    let probe_id = "__resolve_probe";
    let raw = "__resolve_raw";
    let std = "__resolve_std";
    session.execute_batch(&format!("drop table if exists {raw}"))?;
    session.execute_batch(&format!("drop table if exists {std}"))?;
    let payload_literal = payload.to_string().replace('\'', "''");
    let source_literal = source.replace('\'', "''");
    session.execute_batch(&format!(
        "create table {raw} as select
             '{probe_id}' as id,
             '{source_literal}' as source_system,
             '{probe_id}' as source_record_id,
             '{payload_literal}'::json as payload,
             '{probe_id}' as content_hash,
             false as is_deleted,
             cast(null as timestamp) as deleted_at,
             '{probe_id}' as ingest_batch_id,
             now() as ingested_at,
             now() as created_at,
             now() as updated_at"
    ))?;
    build_int_std_records(session, config, raw, LEXICON, std, "1970-01-01 00:00:00")?;

    let probe = session.register_table(std, std)?;
    let corpus_concat = dedupe_core::predict::materialize_concat(session, &corpus)?;
    let probe_concat = dedupe_core::predict::materialize_concat(session, &probe)?;
    let tf_tables = build_frozen_tf_tables(
        session,
        &settings,
        model.tf_csv_path,
        model.model_version,
        model.tf_snapshot_id,
    )?;
    let scored = dedupe_core::incremental::predict_between(
        session,
        &settings,
        &corpus_concat,
        &probe_concat,
        &tf_tables,
        None,
    )?;

    // Project the corpus side, its current entity, and the evidence. The
    // membership join is a LEFT join: a record that has not been reconciled yet
    // is still a legitimate candidate, it just has no entity id to report.
    let uid = &settings.unique_id_column;
    let evidence = evidence_json(&settings);
    let review_low = config.thresholds.review_low;
    let auto_merge = config.thresholds.auto_merge;
    let membership_ok = session
        .register_lake_table("entity_membership", "entity_membership")
        .is_ok();
    // Newest membership row per record. `entity_membership` carries
    // `assigned_at` + `run_id` rather than an active flag, so a record that has
    // been through several runs has several rows and the latest is the one that
    // describes it now. A plain join would report an entity the record has since
    // left.
    let membership_join = if membership_ok {
        format!(
            "left join (
                 select record_key, entity_id,
                        row_number() over (
                            partition by record_key
                            order by assigned_at desc, run_id desc
                        ) as rn
                 from {ENTITY_MEMBERSHIP}
             ) m on m.record_key = corpus_key and m.rn = 1"
        )
    } else {
        String::new()
    };
    let entity_select = if membership_ok {
        "m.entity_id"
    } else {
        "cast(null as varchar)"
    };
    let rows = session.query_scratch_strings(&format!(
        "with scored as (
             select case when \"{uid}_l\" = '{probe_id}' then \"{uid}_r\" else \"{uid}_l\" end
                        as corpus_key,
                    match_probability,
                    {evidence} as evidence
             from {preds}
             where \"{uid}_l\" <> \"{uid}_r\" and match_probability >= {review_low}
         )
         select corpus_key,
                {entity_select},
                cast(match_probability as varchar),
                cast(evidence as varchar)
         from scored
         {membership_join}
         order by match_probability desc, corpus_key
         limit {limit}",
        preds = scored.physical,
    ))?;

    let candidates: Vec<ResolveCandidate> = rows
        .iter()
        .map(|r| {
            let probability: f64 = r[2].parse().unwrap_or(0.0);
            ResolveCandidate {
                record_key: r[0].clone(),
                entity_id: Some(r[1].clone()).filter(|v| !v.is_empty()),
                match_probability: probability,
                band: band_for(probability, review_low, auto_merge).to_string(),
                evidence: serde_json::from_str(&r[3]).unwrap_or(serde_json::Value::Null),
            }
        })
        .collect();

    // Clean up the probe tables so a reused session cannot leak one resolve into
    // the next.
    session.execute_batch(&format!("drop table if exists {raw}"))?;
    session.execute_batch(&format!("drop table if exists {std}"))?;

    let decision = candidates
        .first()
        .map_or("reject", |c| c.band.as_str())
        .to_string();
    Ok(ResolveResult {
        candidates,
        decision,
        snapshot: session.latest_snapshot_version().unwrap_or(0),
    })
}

/// Which threshold band a probability falls in. The same three-band policy the
/// batch path applies, so an online decision and a batch one agree.
fn band_for(probability: f64, review_low: f64, auto_merge: f64) -> &'static str {
    if probability >= auto_merge {
        "auto_merge"
    } else if probability >= review_low {
        "review"
    } else {
        "reject"
    }
}

/// What a run would cost before it runs: how many pairs each blocking rule
/// generates, and the largest blocks the first rule produces.
///
/// This is the answer to the competitor's hard three-hour analysis cap, whose
/// documented workaround is "split your dataset with filters or raise the
/// confidence floor". Both are guesses made blind. Blocking decides the entire
/// cost of a run — the comparison count is what the engine actually works
/// through — so the fix is to show the number first rather than to cap the
/// clock.
///
/// `n_largest` surfaces the other half of the problem: one pathological key
/// (every record with a blank postcode, say) can dominate a run even when the
/// total looks fine. The largest-block list names it.
///
/// Commits nothing. The concat is materialized into the session's scratch space
/// exactly as `preview_match` does, and the lake is only ever read.
///
/// # Errors
/// Propagates model-load / lake-read errors. A blocking rule that is not a pure
/// equi-join has no block breakdown; the per-rule counts are still returned and
/// `largest_blocks` comes back empty rather than failing the whole estimate.
pub fn estimate_blocking(
    session: &Session,
    settings_json: &str,
    rules: &[String],
    n_largest: usize,
) -> Result<BlockingEstimate> {
    let settings = load_model(settings_json)?;
    let input = session.register_lake_table("int_std_records", "int_std_records")?;
    let refs: Vec<(&dedupe_core::session::TableHandle, &str)> = vec![(&input, "int_std_records")];
    let concat = dedupe_core::predict::materialize_concat_multi(session, &refs, &settings)?;

    let counts = dedupe_core::blocking_analysis::count_comparisons_from_blocking_rules(
        session, &settings, &concat, rules,
    )?;
    let per_rule: Vec<BlockingRuleEstimate> = counts
        .iter()
        .map(|c| BlockingRuleEstimate {
            blocking_rule: c.blocking_rule.clone(),
            match_key: c.match_key,
            marginal_pairs: c.marginal_comparison_count,
            cumulative_pairs: c.cumulative_comparison_count,
        })
        .collect();
    let total_possible = counts
        .first()
        .map_or(0, |c| c.total_possible_comparison_count);

    // The largest blocks of the FIRST rule. Only a pure equi-join has blocks to
    // report, and a rule with a filter is not an error — it just has no
    // breakdown, so an estimate is still returned.
    let largest_blocks = match rules.first() {
        Some(rule) if n_largest > 0 => dedupe_core::blocking_analysis::n_largest_blocks(
            session, &settings, &concat, rule, n_largest,
        )
        .map(|blocks| {
            blocks
                .into_iter()
                .map(|b| LargestBlockEstimate {
                    key_values: b.key_values,
                    pairs: b.block_count,
                })
                .collect()
        })
        .unwrap_or_default(),
        _ => Vec::new(),
    };

    Ok(BlockingEstimate {
        total_possible_pairs: total_possible,
        candidate_pairs: per_rule.last().map_or(0, |r| r.cumulative_pairs),
        per_rule,
        largest_blocks,
    })
}

/// One blocking rule's contribution to the candidate set.
#[derive(Debug, Clone, serde::Serialize)]
pub struct BlockingRuleEstimate {
    pub blocking_rule: String,
    pub match_key: i64,
    /// Pairs this rule adds that no earlier rule already generated — the number
    /// that says whether the rule is pulling its weight.
    pub marginal_pairs: i64,
    pub cumulative_pairs: i64,
}

/// A blocking key whose block is large enough to dominate a run.
#[derive(Debug, Clone, serde::Serialize)]
pub struct LargestBlockEstimate {
    pub key_values: Vec<String>,
    pub pairs: i64,
}

/// What a run would cost, before running it.
#[derive(Debug, Clone, serde::Serialize)]
pub struct BlockingEstimate {
    /// Every pair the corpus could produce — what resolution would cost with no
    /// blocking at all.
    pub total_possible_pairs: i64,
    /// What the declared rules actually generate: the number that decides the
    /// run's cost.
    pub candidate_pairs: i64,
    pub per_rule: Vec<BlockingRuleEstimate>,
    pub largest_blocks: Vec<LargestBlockEstimate>,
}

/// S4.3 match (full mode): score `int_std_records` with FROZEN TF against the
/// committed model and MERGE the canonical pairs at or above `review_low` into
/// `lake.main.match_scores` (S5 schema: canonical endpoints, model/snapshot keys,
/// both content hashes, the `evidence` waterfall, `is_active`). Learned params
/// stay frozen; `retain_intermediate` is turned on (scoring_settings, S4.3.4) so
/// the evidence columns survive.
///
/// # Errors
/// Propagates model-load / predict / lake DDL+MERGE errors.
pub fn run_match(
    session: &Session,
    config: &Config,
    model: &MatchModel,
    review_low: f64,
    run_id: &str,
    scored_at: &str,
) -> Result<()> {
    let mut settings = load_model(model.settings_json)?;
    settings.retain_intermediate = true;

    // The matched corpus is the person or Account standardized table, by
    // entity_type. MATCH_SCORES (the merge target below) is shared + entity-agnostic.
    let (std_unqual, std_qual) = if config.entity_type.is_person() {
        ("int_std_records", INT_STD_RECORDS)
    } else {
        ("acct_int_std_records", ACCT_INT_STD_RECORDS)
    };
    let input = session.register_lake_table(std_unqual, std_unqual)?;
    let tf_tables = build_frozen_tf_tables(
        session,
        &settings,
        model.tf_csv_path,
        model.model_version,
        model.tf_snapshot_id,
    )?;
    let predictions = run_predict_with_tf(session, &settings, &input, &tf_tables, None)?;

    // A full pass recomputes every pair, so everything this model previously
    // scored is about to be superseded: deactivate it all, then let the MERGE
    // bring back whatever still clears the threshold. Without this, a score can
    // only ever be created — so a pair that drops below `review_low`, or whose
    // record was deleted from the corpus, keeps an active row forever, and
    // reconcile rebuilds its node universe from edge endpoints (see
    // `run_reconcile`), which resurrects deleted records into their old entity.
    invalidate_superseded_scores(session, model, run_id, scored_at, StaleScope::EveryActive)?;
    // And retire any OTHER model's scores, so activating a new version takes
    // effect instead of being unioned with the one it replaced.
    retire_other_models(
        session,
        &[(model.model_version, model.tf_snapshot_id)],
        run_id,
        scored_at,
    )?;
    stage_and_upsert_scores(
        session,
        &ScorePass {
            settings: &settings,
            model,
            review_low,
            run_id,
            scored_at,
            std_qual,
            predictions: std::slice::from_ref(&predictions.physical),
        },
    )?;
    // A full pass scored the whole corpus, so the whole corpus is what it may
    // claim.
    sync_scored_ledger(session, model, run_id, scored_at, std_qual, std_qual)?;
    Ok(())
}

/// Retire the scores of every model this run is not using, so that activating a
/// new model version actually takes effect. Returns the number retired.
///
/// Scores are keyed by `(pair, model_version, tf_snapshot_id)`, so a run under a
/// new model writes NEW rows and leaves the previous model's rows `is_active`.
/// Neither edge read filters by model — `auto_merge_edges_live` (what reconcile
/// clusters from) and `active_page` (what the duplicates surface reads) both take
/// every active row — so without this the two models' verdicts are unioned,
/// silently and for good. The sharp end of that: a new model built to be
/// STRICTER could not change anything, because the old model's looser edges went
/// on merging the records.
///
/// `live` is the set of model identities this run scores with. It is one today
/// and a list because an ensemble is a list — the plural shape is the point, not
/// speculation.
fn retire_other_models(
    session: &Session,
    live: &[(&str, &str)],
    run_id: &str,
    scored_at: &str,
) -> Result<usize> {
    create_ddl_table(session, "match_scores")?;
    assert!(
        !live.is_empty(),
        "a run always scores with at least one model; an empty live set would \
         retire every score in the lake"
    );
    let run_id = run_id.replace('\'', "''");
    let scored_at = scored_at.replace('\'', "''");
    let rows: Vec<String> = live
        .iter()
        .map(|(mv, snap)| {
            format!(
                "('{}', '{}')",
                mv.replace('\'', "''"),
                snap.replace('\'', "''")
            )
        })
        .collect();
    for t in ["__live_models", "__retire_stamp"] {
        session.execute_batch(&format!("drop table if exists {t}"))?;
    }
    session.execute_batch(&format!(
        "create temp table __live_models as select * from (values {}) \
         as t(model_version, tf_snapshot_id)",
        rows.join(", ")
    ))?;
    session.execute_batch(&format!(
        "create temp table __retire_stamp as
         select '{run_id}' as run_id, cast('{scored_at}' as timestamp) as invalidated_at"
    ))?;
    let n = er_entities::match_score::update::retire_other_models(session.connection())?;
    for t in ["__live_models", "__retire_stamp"] {
        session.execute_batch(&format!("drop table {t}"))?;
    }
    Ok(n)
}

/// Which previously-scored records a pass is about to supersede.
enum StaleScope<'a> {
    /// A full pass recomputes the whole corpus: every active score for this
    /// model identity is superseded.
    EveryActive,
    /// An incremental pass recomputes only the pairs touching these records —
    /// the new, the changed and the departed.
    Records(&'a str),
}

/// Deactivate the scores a pass is about to recompute, so that `is_active`
/// means "this is what the model currently says" rather than "this was true
/// once". Returns the number of rows deactivated.
///
/// The ordering matters: invalidate first, re-score second. A pair that still
/// clears the threshold is re-activated by the MERGE in the same pass; one that
/// no longer does, or whose endpoint has left the corpus, correctly stays
/// inactive.
fn invalidate_superseded_scores(
    session: &Session,
    model: &MatchModel,
    run_id: &str,
    scored_at: &str,
    scope: StaleScope,
) -> Result<usize> {
    // Nothing to invalidate before the first pass creates the relation.
    create_ddl_table(session, "match_scores")?;
    let run_id = run_id.replace('\'', "''");
    let scored_at = scored_at.replace('\'', "''");
    let (mv, snap) = (
        model.model_version.replace('\'', "''"),
        model.tf_snapshot_id.replace('\'', "''"),
    );
    // Both scopes stage the same shape — the record keys whose pairs are being
    // superseded — so one declared statement serves both. For a full pass that
    // is every endpoint this model has an active score for, which is exactly
    // "all of them".
    let keys = match scope {
        StaleScope::EveryActive => format!(
            "select rec_a_key as record_key from {MATCH_SCORES}
              where is_active and model_version = '{mv}' and tf_snapshot_id = '{snap}'
             union
             select rec_b_key as record_key from {MATCH_SCORES}
              where is_active and model_version = '{mv}' and tf_snapshot_id = '{snap}'"
        ),
        StaleScope::Records(sql) => sql.to_string(),
    };
    session.execute_batch("drop table if exists __match_stale")?;
    session.execute_batch(&format!(
        "create temp table __match_stale as
         select record_key,
                '{mv}' as model_version, '{snap}' as tf_snapshot_id,
                '{run_id}' as run_id, cast('{scored_at}' as timestamp) as invalidated_at
         from ({keys})"
    ))?;
    let n = er_entities::match_score::update::invalidate_stale(session.connection())?;
    session.execute_batch("drop table __match_stale")?;
    Ok(n)
}

/// Write one or more prediction tables into `match_scores` through the declared
/// MERGE: canonicalise the endpoints, keep the highest-probability row per pair,
/// threshold at `review_low`, join both endpoint content hashes, and render the
/// evidence — exactly er.matching.full's `_selected_predictions_sql` +
/// `_source_sql`.
///
/// `predictions` is a list rather than a single table because an incremental
/// pass scores in two shots (existing x delta, then delta x delta) whose results
/// are disjoint by construction: a pair's two endpoints are either both in the
/// delta or not, so no pair can appear in both and the union needs no
/// cross-table deduplication beyond the per-pair `distinct on` already here.
struct ScorePass<'a> {
    settings: &'a Settings,
    model: &'a MatchModel<'a>,
    review_low: f64,
    run_id: &'a str,
    scored_at: &'a str,
    /// The qualified standardized relation the content hashes are joined from.
    std_qual: &'a str,
    /// Physical names of the prediction tables to union.
    predictions: &'a [String],
}

fn stage_and_upsert_scores(session: &Session, pass: &ScorePass) -> Result<()> {
    let ScorePass {
        settings,
        model,
        review_low,
        run_id,
        scored_at,
        std_qual,
        predictions,
    } = *pass;
    let spec = er_schema::model::all_specs()
        .into_iter()
        .find(|s| s.name == "match_scores")
        .ok_or_else(|| anyhow::anyhow!("match_scores spec missing from S5 registry"))?;
    session.execute_batch(
        &er_schema::model::create_table_sql(&spec).map_err(|e| anyhow::anyhow!("{e}"))?,
    )?;
    if predictions.is_empty() {
        return Ok(());
    }

    let uid = &settings.unique_id_column;
    let evidence = evidence_json(settings);
    let run_id = run_id.replace('\'', "''");
    let scored_at = scored_at.replace('\'', "''");
    let (mv, snap) = (
        model.model_version.replace('\'', "''"),
        model.tf_snapshot_id.replace('\'', "''"),
    );
    let raw = predictions
        .iter()
        .map(|p| {
            format!(
                "select least(\"{uid}_l\", \"{uid}_r\") as rec_a_key,
                        greatest(\"{uid}_l\", \"{uid}_r\") as rec_b_key,
                        match_probability, {evidence} as evidence, rowid as _rid
                 from {p}
                 where \"{uid}_l\" <> \"{uid}_r\" and match_probability >= {review_low}"
            )
        })
        .collect::<Vec<_>>()
        .join("\n             union all\n             ");
    session.execute_batch(&format!(
        "create temp table __match_src as
         with scored as (
             select distinct on (rec_a_key, rec_b_key)
                    rec_a_key, rec_b_key, match_probability, evidence
             from (
             {raw}
             )
             order by rec_a_key, rec_b_key, match_probability desc, _rid
         )
         select s.rec_a_key, s.rec_b_key, s.match_probability,
                '{mv}' as model_version, '{snap}' as tf_snapshot_id,
                ra.content_hash as rec_a_content_hash, rb.content_hash as rec_b_content_hash,
                s.evidence, true as is_active,
                cast(null as timestamp) as invalidated_at, cast(null as varchar) as invalidated_run_id,
                '{run_id}' as run_id, cast('{scored_at}' as timestamp) as scored_at
         from scored s
         join {std_qual} ra on ra.record_key = s.rec_a_key
         join {std_qual} rb on rb.record_key = s.rec_b_key",
    ))?;

    // S4.3.4 single write: the declared MERGE on the logical key, over the
    // model-derived __match_src staging. A re-scored pair is rewritten in place
    // (clearing any invalidation); a new pair is inserted.
    er_entities::match_score::update::upsert_from_scored(session.connection())?;
    session.execute_batch("drop table __match_src")?;
    Ok(())
}

/// Record what this match pass scored, so the next one can tell new from
/// unchanged. Written at the END of a successful pass and never before, which is
/// what makes the ledger safe: a run that lands records and then dies while
/// scoring leaves the ledger untouched, so the next pass still treats those
/// records as unscored.
///
/// `scored` is the relation holding the records this pass actually scored, and
/// it is NOT always the whole corpus — a segment pass scores a slice. Claiming
/// the corpus would make the ledger assert that an unscored record had been
/// scored, and the next pass would believe it and never score it. The retirement
/// half compares against the whole corpus instead (`std_qual`), because what a
/// pass may claim to have scored and what still exists are two different sets.
fn sync_scored_ledger(
    session: &Session,
    model: &MatchModel,
    run_id: &str,
    scored_at: &str,
    std_qual: &str,
    scored: &str,
) -> Result<()> {
    create_ddl_table(session, "scored_records")?;
    let run_id = run_id.replace('\'', "''");
    let scored_at = scored_at.replace('\'', "''");
    let (mv, snap) = (
        model.model_version.replace('\'', "''"),
        model.tf_snapshot_id.replace('\'', "''"),
    );
    session.execute_batch("drop table if exists __scored_src")?;
    session.execute_batch("drop table if exists __scored_live")?;
    session.execute_batch(&format!(
        "create temp table __scored_src as
         select record_key, content_hash,
                '{mv}' as model_version, '{snap}' as tf_snapshot_id,
                '{run_id}' as run_id, cast('{scored_at}' as timestamp) as scored_at
         from {scored}"
    ))?;
    session.execute_batch(&format!(
        "create temp table __scored_live as select record_key from {std_qual}"
    ))?;
    er_entities::scored_record::update::upsert_from_std(session.connection())?;
    er_entities::scored_record::delete::absent_from_std(session.connection())?;
    session.execute_batch("drop table __scored_src")?;
    session.execute_batch("drop table __scored_live")?;
    Ok(())
}

/// What an incremental match pass did, for the ledger and for operators who
/// need to know whether they got the cheap path.
#[derive(Debug, Clone, serde::Serialize)]
pub struct MatchDelta {
    /// `incremental` when the two-pass path ran, `full` when the pass had to
    /// score the whole corpus (the honest answer, not the asked-for one).
    pub mode: &'static str,
    /// Records in the corpus after this pass.
    pub corpus_records: usize,
    /// Records that were new or changed — the ones whose pairs were scored.
    pub delta_records: usize,
    /// Previously-active scores deactivated because an endpoint changed or left.
    pub invalidated_scores: usize,
    /// Why the pass could not be incremental, when it could not be.
    pub full_reason: Option<String>,
    /// The predicate this pass was restricted to, when it was. The engine only
    /// ever sees the predicate; the segment NAME belongs to the layer that
    /// resolved it against the config, which is where it is logged.
    pub scope_predicate: Option<String>,
}

/// S4.3.4 match (incremental mode): score only the pairs that could have
/// changed, and reuse the rest.
///
/// The two passes are `predict_between(unchanged, delta)` and
/// `predict_within(delta)`; their union is every pair with at least one endpoint
/// in the delta. Pairs between two unchanged records are not re-scored — they
/// are already in `match_scores` and, because the model and the term-frequency
/// snapshot are both frozen, re-scoring them could only reproduce the number
/// already stored. That is what makes this equal to a full pass rather than an
/// approximation of one, and it is the half of DesignDoc §4.3.4 that was
/// specified but never built: `predict_between` / `predict_within` existed and
/// were golden-tested, and nothing in the pipeline called them.
///
/// `predict_within` over the delta is load-bearing on its own: without it, two
/// new records that match each other but nothing in the corpus would never be
/// compared, so "a batch contains a new duplicate pair" would silently resolve
/// to two entities.
///
/// The delta comes from the `scored_records` ledger, not from the ingest batch
/// id: a run that lands records and then fails before scoring them would leave
/// them looking "already ingested" to any batch-based definition, and they would
/// never be scored at all. Comparing the corpus against what was actually
/// scored cannot have that hole — and it makes a *changed* record (same key, new
/// content hash) and a *departed* one (tombstoned, so standardize drops it)
/// fall out of the same comparison.
///
/// Falls back to a full pass, saying so in [`MatchDelta::full_reason`], when
/// there is nothing to reuse: no ledger yet, or a ledger written under a
/// different model identity (whose scores are different rows in `match_scores`
/// anyway).
///
/// # Errors
/// Propagates model-load / predict / lake DDL+MERGE errors.
pub fn run_match_incremental(
    session: &Session,
    config: &Config,
    model: &MatchModel,
    review_low: f64,
    run_id: &str,
    scored_at: &str,
) -> Result<MatchDelta> {
    run_match_partial(session, config, model, review_low, run_id, scored_at, None)
}

/// S4.3.4 match restricted to a named segment (#54): score `segment × corpus`
/// and `segment × segment`, reuse everything else.
///
/// This is the answer to "my corpus is too big for one run" — the problem the
/// incumbent answers with a run-duration cap and the advice to split your data.
/// Splitting the *corpus* loses every cross-segment duplicate and strands the
/// records outside the slice; splitting the *work* does not. The segment is the
/// side that gets re-scored, and it is scored against the whole corpus, so no
/// pair goes unexamined and `match_scores` ends up where a full run would leave
/// it. Run each segment in turn and the corpus is covered.
///
/// The scope is widened to include anything new or changed wherever it lives.
/// That is deliberate: the alternative is to score a slice and then write a
/// ledger claiming the records outside it were scored, which the next pass would
/// believe. A pass that leaves no record unscored cannot lie in that way, and
/// the extra cost is the batch, which is the cheap part.
///
/// `predicate` is a SQL boolean over the standardized columns. It must come from
/// the published config's `segments` block, which is parsed and column-checked at
/// publish — never from a request.
///
/// # Errors
/// Propagates model-load / predict / lake DDL+MERGE errors, including an invalid
/// segment predicate (as the lake's own SQL error).
pub fn run_match_segment(
    session: &Session,
    config: &Config,
    model: &MatchModel,
    review_low: f64,
    run_id: &str,
    scored_at: &str,
    predicate: &str,
) -> Result<MatchDelta> {
    run_match_partial(
        session,
        config,
        model,
        review_low,
        run_id,
        scored_at,
        Some(predicate),
    )
}

fn run_match_partial(
    session: &Session,
    config: &Config,
    model: &MatchModel,
    review_low: f64,
    run_id: &str,
    scored_at: &str,
    segment: Option<&str>,
) -> Result<MatchDelta> {
    let std_qual = if config.entity_type.is_person() {
        INT_STD_RECORDS
    } else {
        ACCT_INT_STD_RECORDS
    };
    create_ddl_table(session, "scored_records")?;
    let state = er_entities::scored_record::read::state(session.connection())?;
    let ledger = state.first();
    let scored_rows = ledger.and_then(|s| s.n).unwrap_or(0);
    let identities = ledger.and_then(|s| s.identities).unwrap_or(0);
    let same_model = ledger.is_some_and(|s| {
        identities == 1
            && s.model_version.as_deref() == Some(model.model_version)
            && s.tf_snapshot_id.as_deref() == Some(model.tf_snapshot_id)
    });

    let full_reason = if scored_rows == 0 {
        Some("nothing has been scored yet".to_string())
    } else if !same_model {
        Some(format!(
            "the corpus was scored under a different model identity than {}/{}",
            model.model_version, model.tf_snapshot_id
        ))
    } else {
        None
    };
    if let Some(reason) = full_reason {
        // A segment cannot help when there is nothing to reuse: the whole corpus
        // has to be scored either way, so scope it to everything rather than
        // leave the records outside the slice unscored with a ledger that says
        // otherwise.
        run_match(session, config, model, review_low, run_id, scored_at)?;
        return Ok(MatchDelta {
            mode: er_versions::MODE_FULL,
            corpus_records: count_rows(session, std_qual)?,
            delta_records: count_rows(session, std_qual)?,
            invalidated_scores: 0,
            full_reason: Some(reason),
            scope_predicate: segment.map(ToOwned::to_owned),
        });
    }

    let mut settings = load_model(model.settings_json)?;
    settings.retain_intermediate = true;

    // The delta and its complement, as projections of the standardized corpus.
    // A record is in the delta when the ledger has no row claiming that exact
    // (record_key, content_hash) — which covers both "never scored" and
    // "scored, then edited" — or when a segment predicate selects it.
    let delta_std = "__inc_delta_std";
    let kept_std = "__inc_kept_std";
    for t in [delta_std, kept_std] {
        session.execute_batch(&format!("drop table if exists {t}"))?;
    }
    let matched = format!(
        "select 1 from {SCORED_RECORDS} l
          where l.record_key = s.record_key and l.content_hash = s.content_hash"
    );
    // `in scope` and `not in scope` are written as negations of one expression so
    // the two sides cannot drift into overlapping or leaving a gap. A predicate
    // that is NULL for a row counts as out of scope, which `not (p)` would make
    // NULL rather than true — hence the explicit `coalesce`.
    let in_scope = match segment {
        Some(p) => format!("coalesce({p}, false)"),
        None => "false".to_string(),
    };
    session.execute_batch(&format!(
        "create table {delta_std} as
         select s.* from {std_qual} s where not exists ({matched}) or {in_scope}"
    ))?;
    session.execute_batch(&format!(
        "create table {kept_std} as
         select s.* from {std_qual} s where exists ({matched}) and not {in_scope}"
    ))?;
    let delta_records = count_rows(session, delta_std)?;
    let corpus_records = delta_records + count_rows(session, kept_std)?;

    // Every score that is about to be superseded: pairs touching a delta record,
    // and pairs touching a record the corpus no longer has at all (a tombstone
    // standardize dropped). The second half is why a deletion stops resurrecting
    // itself through reconcile's edge endpoints.
    let mut invalidated = invalidate_superseded_scores(
        session,
        model,
        run_id,
        scored_at,
        StaleScope::Records(&format!(
            "select record_key from {delta_std}
             union
             select record_key from {SCORED_RECORDS}
              where record_key not in (select record_key from {std_qual})"
        )),
    )?;
    // Other models' scores go whether or not this pass touches their pairs: they
    // are not from a model this run uses, so they must not keep clustering.
    invalidated += retire_other_models(
        session,
        &[(model.model_version, model.tf_snapshot_id)],
        run_id,
        scored_at,
    )?;

    if delta_records > 0 {
        let tf_tables = build_frozen_tf_tables(
            session,
            &settings,
            model.tf_csv_path,
            model.model_version,
            model.tf_snapshot_id,
        )?;
        let delta = session.register_table(delta_std, delta_std)?;
        let delta_concat = dedupe_core::predict::materialize_concat(session, &delta)?;
        let mut predictions = Vec::new();
        // Pass 1 — new/changed against the rest of the corpus. Skipped when the
        // whole corpus is the delta, since there is no other side to join to and
        // a cross join against nothing is an empty table either way.
        if corpus_records > delta_records {
            let kept = session.register_table(kept_std, kept_std)?;
            let kept_concat = dedupe_core::predict::materialize_concat(session, &kept)?;
            let between = dedupe_core::incremental::predict_between(
                session,
                &settings,
                &kept_concat,
                &delta_concat,
                &tf_tables,
                None,
            )?;
            predictions.push(between.physical.clone());
        }
        // Pass 2 — new/changed against each other.
        let within = dedupe_core::incremental::predict_within(
            session,
            &settings,
            &delta_concat,
            &tf_tables,
            None,
        )?;
        predictions.push(within.physical.clone());

        stage_and_upsert_scores(
            session,
            &ScorePass {
                settings: &settings,
                model,
                review_low,
                run_id,
                scored_at,
                std_qual,
                predictions: &predictions,
            },
        )?;
    }

    // The ledger earns rows for the delta only — the records this pass actually
    // scored. Claiming the whole corpus would be harmless for an ordinary
    // incremental pass (the rest already have matching rows) but a lie for a
    // segment pass, and the next pass would act on it.
    sync_scored_ledger(session, model, run_id, scored_at, std_qual, delta_std)?;
    for t in [delta_std, kept_std] {
        session.execute_batch(&format!("drop table if exists {t}"))?;
    }
    Ok(MatchDelta {
        mode: er_versions::MODE_INCREMENTAL,
        corpus_records,
        delta_records,
        invalidated_scores: invalidated,
        full_reason: None,
        scope_predicate: segment.map(ToOwned::to_owned),
    })
}

/// `count(*)` over a relation, for the delta report.
fn count_rows(session: &Session, relation: &str) -> Result<usize> {
    let rows = session.query_scratch_strings(&format!("select count(*) from {relation}"))?;
    Ok(rows
        .first()
        .and_then(|r| r.first())
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(0))
}

/// One candidate pair from a score-only preview: canonical endpoints, the
/// probability the committed model gives them, and the evidence waterfall JSON.
pub struct PreviewPair {
    pub rec_a_key: String,
    pub rec_b_key: String,
    pub match_probability: f64,
    pub evidence: String,
}

/// The result of a read-only match preview (S4.3 score-only mode, frontend §7.16):
/// the pairs the committed model scores at/above `review_low`, bucketed by the
/// config thresholds, plus a capped sample carrying the evidence waterfall.
pub struct MatchPreview {
    pub total: usize,
    pub would_auto_merge: usize,
    pub would_review: usize,
    pub sample: Vec<PreviewPair>,
}

/// Score `int_std_records` with the frozen model EXACTLY as `run_match` does, but
/// WITHOUT committing anything: no `match_scores` DDL, no MERGE, no
/// reconcile/golden. The scored pairs live only in a session-local temp table
/// that is dropped before return, so nothing reaches the lake catalog and no
/// determinism-contract artifact (`config_hash` / `content_hash` / golden
/// fixtures) is touched — this is a pure read. Pairs are canonicalised,
/// top-probability-per-pair, thresholded at `review_low`; `would_auto_merge`
/// counts probability >= `auto_merge`, `would_review` the `[review_low,
/// auto_merge)` band; `sample` is the top `sample_limit` pairs by probability
/// with the evidence waterfall.
///
/// # Errors
/// Propagates model-load / predict / lake query errors.
pub fn preview_match(
    session: &Session,
    model: &MatchModel,
    review_low: f64,
    auto_merge: f64,
    sample_limit: usize,
) -> Result<MatchPreview> {
    let mut settings = load_model(model.settings_json)?;
    settings.retain_intermediate = true;

    let input = session.register_lake_table("int_std_records", "int_std_records")?;
    let tf_tables = build_frozen_tf_tables(
        session,
        &settings,
        model.tf_csv_path,
        model.model_version,
        model.tf_snapshot_id,
    )?;
    let predictions = run_predict_with_tf(session, &settings, &input, &tf_tables, None)?;

    let uid = &settings.unique_id_column;
    let evidence = evidence_json(&settings);
    // The same canonical top-per-pair selection `run_match` commits, minus the
    // content-hash joins and the committed CREATE/MERGE. A session-local TEMP
    // table (never the lake catalog), dropped below — nothing is committed.
    session.execute_batch(&format!(
        "create temp table __preview_src as
         with scored as (
             select distinct on (rec_a_key, rec_b_key)
                    rec_a_key, rec_b_key, match_probability, evidence
             from (
                 select least(\"{uid}_l\", \"{uid}_r\") as rec_a_key,
                        greatest(\"{uid}_l\", \"{uid}_r\") as rec_b_key,
                        match_probability, {evidence} as evidence, rowid as _rid
                 from {preds}
                 where \"{uid}_l\" <> \"{uid}_r\" and match_probability >= {review_low}
             )
             order by rec_a_key, rec_b_key, match_probability desc, _rid
         )
         select * from scored",
        preds = predictions.physical,
    ))?;

    let counts = session.query_scratch_strings(&format!(
        "select count(*), \
                count(*) filter (where match_probability >= {auto_merge}), \
                count(*) filter (where match_probability >= {review_low} and match_probability < {auto_merge}) \
         from __preview_src"
    ))?;
    let row = counts
        .first()
        .ok_or_else(|| anyhow::anyhow!("preview counts returned no row"))?;
    let total = row[0].parse::<usize>().unwrap_or(0);
    let would_auto_merge = row[1].parse::<usize>().unwrap_or(0);
    let would_review = row[2].parse::<usize>().unwrap_or(0);

    let sample = session
        .query_scratch_strings(&format!(
            "select rec_a_key, rec_b_key, match_probability, evidence from __preview_src \
             order by match_probability desc, rec_a_key, rec_b_key limit {sample_limit}"
        ))?
        .into_iter()
        .map(|r| PreviewPair {
            rec_a_key: r[0].clone(),
            rec_b_key: r[1].clone(),
            match_probability: r[2].parse::<f64>().unwrap_or(0.0),
            evidence: r[3].clone(),
        })
        .collect();

    session.execute_batch("drop table __preview_src")?;
    Ok(MatchPreview {
        total,
        would_auto_merge,
        would_review,
        sample,
    })
}

/// SQL single-quoted literal escape.
fn esc(s: &str) -> String {
    s.replace('\'', "''")
}

/// Create a DDL-owned S5 relation in the lake (idempotent `CREATE TABLE IF NOT
/// EXISTS` from the er-schema registry).
fn create_ddl_table(session: &Session, name: &str) -> Result<()> {
    let spec = er_schema::model::all_specs()
        .into_iter()
        .find(|s| s.name == name)
        .ok_or_else(|| anyhow::anyhow!("{name} spec missing from S5 registry"))?;
    session.execute_batch(
        &er_schema::model::create_table_sql(&spec).map_err(|e| anyhow::anyhow!("{e}"))?,
    )?;
    Ok(())
}

/// Record the `runs` row at the start of a run (status `running`): the run
/// ledger the read API's `GET /runs` surfaces. `started_at` is the run's logical
/// timestamp; `code_version` is this crate's version.
///
/// The mode written is the mode the run is actually executing, which is why it
/// comes from `inputs` rather than being hardcoded: this used to write `"full"`
/// for every run, including incremental ones, so the ledger could not be used to
/// tell them apart.
///
/// # Errors
/// Propagates lake DDL / insert errors.
pub fn write_run_start(session: &Session, config: &Config, inputs: &PipelineInputs) -> Result<()> {
    write_run_row(
        session,
        config,
        inputs.run_id,
        inputs.run_at,
        inputs.mode,
        inputs.model.model_version,
        inputs.model.tf_snapshot_id,
        Some(&lexicon_fingerprint(inputs.lexicon)),
        inputs.rebuild_reason,
    )
}

/// The lexicon's contribution to the run fingerprint. A change here changes
/// which names are treated as variants of each other, so a corpus scored under
/// one lexicon cannot be incrementally extended under another.
fn lexicon_fingerprint(pairs: &[(String, String)]) -> String {
    er_std::lexicon::lexicon_hash(pairs.iter().map(|(a, b)| (a.as_str(), b.as_str())))
}

/// The five `runs` columns the drift guard compares, read from the last
/// SUCCEEDED run. `None` when nothing has succeeded yet (the first run is never
/// guarded).
///
/// # Errors
/// Propagates the lake read error; a missing `runs` table reads as `None`.
pub fn prior_fingerprint(session: &Session) -> Result<Option<er_versions::RunFingerprint>> {
    let rows = match er_entities::run::read::latest_fingerprint(session.connection()) {
        Ok(rows) => rows,
        Err(e) if is_missing_table(&e.to_string()) => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    Ok(rows
        .into_iter()
        .next()
        .map(|r| er_versions::RunFingerprint {
            config_hash: r.config_hash,
            model_version: r.model_version,
            std_version: r.std_version,
            survivorship_version: r.survivorship_version,
            lexicon_hash: r.lexicon_hash,
        }))
}

/// The fingerprint this run would carry, for comparison against
/// [`prior_fingerprint`].
#[must_use]
pub fn current_fingerprint(
    config: &Config,
    model_version: &str,
    lexicon: &[(String, String)],
) -> er_versions::RunFingerprint {
    er_versions::RunFingerprint {
        config_hash: er_config::config_hash(config),
        model_version: Some(model_version.to_string()),
        std_version: config.versions.std_version.clone(),
        survivorship_version: config.versions.survivorship_version.clone(),
        lexicon_hash: Some(lexicon_fingerprint(lexicon)),
    }
}

/// Insert one `running` row into the `runs` ledger with the given `mode` and the
/// model identity the run scored against. `code_version` is this crate's version.
#[allow(clippy::too_many_arguments)]
fn write_run_row(
    session: &Session,
    config: &Config,
    run_id: &str,
    started_at: &str,
    mode: &str,
    model_version: &str,
    tf_snapshot_id: &str,
    lexicon_hash: Option<&str>,
    rebuild_reason: Option<&str>,
) -> Result<()> {
    create_ddl_table(session, "runs")?;
    er_entities::run::create::start(
        session.connection(),
        run_id,
        &config.tenant,
        mode,
        started_at,
        &er_config::config_hash(config),
        model_version,
        tf_snapshot_id,
        &config.versions.std_version,
        &config.versions.survivorship_version,
        lexicon_hash,
        env!("CARGO_PKG_VERSION"),
        rebuild_reason,
    )?;
    Ok(())
}

/// Write this source's `ingest_batches` receipt row (S4.1): the
/// new/changed/unchanged/tombstone/resurrected breakdown the read API's
/// `GET /imports` surfaces, computed by [`er_ingest::landing::batch_receipt`]
/// against the source's prior landed state. The receipt's `ingest_batch_id`
/// (its primary key) is `{run_id}:{source}` so each source in a run gets a
/// distinct row, while `raw_records` keeps the plain `run_id` batch id the
/// receipt query scopes on. `path` is the delivery's drop path — the source
/// name in this in-process seam; the real dispatcher supplies the object path.
///
/// # Errors
/// Propagates the receipt query / lake DDL+insert errors.
pub fn write_ingest_batch(
    session: &Session,
    run_id: &str,
    source: &str,
    path: &str,
    full_refresh: bool,
    created_at: &str,
) -> Result<()> {
    let r = er_ingest::landing::batch_receipt(session, source, run_id)?;
    create_ddl_table(session, "ingest_batches")?;
    er_entities::ingest_batch::create::receipt(
        session.connection(),
        &format!("{run_id}:{source}"),
        run_id,
        source,
        path,
        r.new,
        r.changed,
        r.unchanged,
        r.tombstone,
        r.resurrected,
        full_refresh,
        created_at,
    )?;
    Ok(())
}

/// Close the `runs` row on success (status `succeeded`, `ended_at` stamped).
///
/// # Errors
/// Propagates the lake update error.
pub fn write_run_end(session: &Session, run_id: &str, ended_at: &str) -> Result<()> {
    er_entities::run::update::finish(session.connection(), ended_at, run_id)?;
    Ok(())
}

/// Apply one drained steward action to the lake (the control plane's
/// `staged_steward_actions`, dispatched when the org's writer lock is free):
///   - `add_assertion` → insert an active assertion on the canonicalized pair
///     (its `assertion_id` is the staged `action_id`, deterministic + stable);
///   - `retract_assertion` → deactivate the named assertion;
///   - `resolve_review` → set the review's status/resolver, and for
///     `match`/`no_match` derive an `always`/`never` assertion from the review's
///     pair so the next reconcile enforces it.
///
/// This records the intent in the lake; ENFORCEMENT (an assertion re-biasing the
/// clustering) happens on the next [`run_reconcile`], which loads the active
/// assertions and force-merges `always` / cuts `never` over the lake graph.
///
/// # Errors
/// Propagates lake DDL / write errors.
pub fn apply_staged_action(
    session: &Session,
    action_id: &str,
    action: &serde_json::Value,
    applied_at: &str,
) -> Result<()> {
    let field = |k: &str| {
        action
            .get(k)
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
    };
    match action.get("type").and_then(serde_json::Value::as_str) {
        Some("add_assertion") => {
            create_ddl_table(session, "assertions")?;
            let (a, b) = (field("a"), field("b"));
            let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
            let note = action.get("note").and_then(serde_json::Value::as_str);
            er_entities::assertion::create::add(
                session.connection(),
                action_id,
                lo,
                hi,
                field("kind"),
                field("created_by"),
                applied_at,
                note,
            )?;
        }
        Some("retract_assertion") => {
            create_ddl_table(session, "assertions")?;
            er_entities::assertion::update::retract(
                session.connection(),
                applied_at,
                field("assertion_id"),
            )?;
        }
        Some("resolve_review") => {
            let review_id = field("review_id");
            let resolution = field("resolution");
            let status = match resolution {
                "match" => "resolved_match",
                "no_match" => "resolved_no_match",
                _ => "dismissed",
            };
            er_entities::review::update::resolve(
                session.connection(),
                status,
                applied_at,
                review_id,
            )?;
            // match/no_match leave an always/never assertion on the review's pair.
            if let Some(kind) = match resolution {
                "match" => Some("always"),
                "no_match" => Some("never"),
                _ => None,
            } {
                create_ddl_table(session, "assertions")?;
                er_entities::assertion::create::from_review(
                    session.connection(),
                    action_id,
                    kind,
                    applied_at,
                    review_id,
                )?;
            }
        }
        // Mass-update: set (override_field) or clear (clear_field) one golden
        // field for one entity. Latest-wins per (entity_id, attribute); the
        // override is applied over golden_records on the next assemble.
        Some(kind @ ("override_field" | "clear_field")) => {
            create_ddl_table(session, "golden_overrides")?;
            let entity_id = field("entity_id");
            let attribute = field("attribute");
            // The 11 overridable golden columns (er-golden SCALAR + ADDRESS).
            const OVERRIDABLE: [&str; 11] = [
                "given_name",
                "family_name",
                "email",
                "phone_e164",
                "birth_date",
                "addr_number",
                "addr_street",
                "addr_unit",
                "addr_city",
                "addr_region",
                "addr_postal",
            ];
            if !entity_id.is_empty() && OVERRIDABLE.contains(&attribute) {
                let value = if kind == "clear_field" {
                    None
                } else {
                    action.get("value").and_then(serde_json::Value::as_str)
                };
                er_entities::golden_override::delete::clear(
                    session.connection(),
                    entity_id,
                    attribute,
                )?;
                er_entities::golden_override::create::set(
                    session.connection(),
                    entity_id,
                    attribute,
                    value,
                    field("created_by"),
                    applied_at,
                    action_id,
                )?;
            }
        }
        // Protect/suppress a record (PR 1.3): one latest-wins active row per
        // (record_key, kind). `suppress` drops the record from reconcile
        // clustering; `protect` forces it master on the read path. Applied only
        // when the overlay table exists, so an empty table is a byte-exact no-op.
        Some(kind @ ("protect_record" | "suppress_record")) => {
            create_ddl_table(session, "record_overlays")?;
            let record_key = field("record_key");
            let overlay_kind = if kind == "protect_record" {
                "protect"
            } else {
                "suppress"
            };
            if !record_key.is_empty() {
                er_entities::record_overlay::delete::clear(
                    session.connection(),
                    record_key,
                    overlay_kind,
                )?;
                er_entities::record_overlay::create::set(
                    session.connection(),
                    record_key,
                    overlay_kind,
                    field("created_by"),
                    applied_at,
                    action_id,
                )?;
            }
        }
        Some("retract_record_overlay") => {
            create_ddl_table(session, "record_overlays")?;
            let record_key = field("record_key");
            let overlay_kind = field("kind"); // "protect" | "suppress"
            if !record_key.is_empty() && matches!(overlay_kind, "protect" | "suppress") {
                er_entities::record_overlay::update::retract(
                    session.connection(),
                    applied_at,
                    record_key,
                    overlay_kind,
                )?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// Populate `review_queue` with the gray-band pairs — active `match_scores`
/// whose probability lands in `[review_low, auto_merge)` — as open `gray_band`
/// pair reviews (S4.5 / design §7.10). Idempotent via a MERGE on the pair +
/// reason: a re-run refreshes probability/waterfall + `last_seen_run_id` and
/// leaves `review_id`/`first_seen_run_id`/`status` intact. The review_id is a
/// deterministic key so the same pair keeps its review across runs.
///
/// NOTE: assertion-adjusted exclusion (pairs an `always`/`never` assertion
/// already decides) is a later refinement; the base path reviews every
/// gray-band pair.
///
/// # Errors
/// Propagates lake DDL / MERGE errors.
pub fn write_reviews(session: &Session, config: &Config, run_id: &str, run_at: &str) -> Result<()> {
    create_ddl_table(session, "review_queue")?;
    // The gray-band source folds into the statement's USING subquery, so the
    // whole upsert is one declared MERGE (no staging table).
    er_entities::review::update::upsert_gray_band(
        session.connection(),
        run_id,
        run_at,
        config.thresholds.review_low,
        config.thresholds.auto_merge,
    )?;
    Ok(())
}

/// Surface never-cut escalations (PR 1.3c) as open `never_unsatisfiable` pair
/// reviews — a `never` pair the fixpoint could not separate without cutting a
/// protected record. Idempotent via a MERGE on the pair + reason, like the
/// gray-band writer, so a re-run keeps the same `review_id`/`first_seen_run_id`
/// and only refreshes `last_seen_run_id`. Called only when a protect overlay is
/// active AND escalations exist, so the base path never touches `review_queue`
/// (the determinism contract holds). `escalations` carries canonicalized
/// (`rec_a_key < rec_b_key`) pairs, matching the review_queue pair invariant.
///
/// # Errors
/// Propagates lake DDL / MERGE errors.
fn write_never_unsatisfiable_reviews(
    session: &Session,
    run_id: &str,
    run_at: &str,
    escalations: &[(String, String, String)],
) -> Result<()> {
    if escalations.is_empty() {
        return Ok(());
    }
    create_ddl_table(session, "review_queue")?;
    // One declared MERGE per escalation (the set is small by construction: a
    // protected record inside an unsatisfiable never is a steward-path rarity).
    for (a, b, _assertion_id) in escalations {
        er_entities::review::update::upsert_never_unsatisfiable(
            session.connection(),
            a,
            b,
            run_id,
            run_at,
        )?;
    }
    Ok(())
}

/// The canonical (smaller, larger) ordering of a record pair.
fn canon(a: &str, b: &str) -> (String, String) {
    if a <= b {
        (a.to_string(), b.to_string())
    } else {
        (b.to_string(), a.to_string())
    }
}

/// Load the active assertions from `lake.main.assertions` (empty if the table
/// isn't there yet — a lake whose steward drain hasn't run).
///
/// # Errors
/// Propagates a non-missing-table query error.
fn load_active_assertions(session: &Session) -> Result<Vec<Assertion>> {
    let rows = match er_entities::assertion::read::active_pairs_live(session.connection()) {
        Ok(rows) => rows,
        Err(e) => {
            if is_missing_table(&e.to_string()) {
                return Ok(Vec::new());
            }
            return Err(e.into());
        }
    };
    Ok(rows
        .into_iter()
        .filter_map(|r| {
            let kind = match r.kind.as_str() {
                "always" => AssertionKind::Always,
                "never" => AssertionKind::Never,
                _ => return None,
            };
            let (a, b) = canon(&r.rec_a_key, &r.rec_b_key);
            Some(Assertion {
                assertion_id: r.id,
                rec_a_key: a,
                rec_b_key: b,
                kind,
                active: true,
            })
        })
        .collect())
}

/// Active suppressed `record_key`s from `lake.main.record_overlays` (empty if the
/// table isn't there yet — a lake whose steward drain hasn't created it). A
/// suppressed record is dropped from every reconcile clustering input.
///
/// # Errors
/// Propagates a non-missing-table query error.
fn load_suppressed_keys(session: &Session) -> Result<BTreeSet<String>> {
    load_overlay_keys(session, "suppress")
}

/// The active overlay record keys of one kind, via the declared live statement
/// (empty when the overlay table hasn't been created yet).
fn load_overlay_keys(session: &Session, kind: &str) -> Result<BTreeSet<String>> {
    match er_entities::record_overlay::read::active_keys_by_kind_live(session.connection(), kind) {
        Ok(rows) => Ok(rows.into_iter().map(|r| r.record_key).collect()),
        Err(e) => {
            if is_missing_table(&e.to_string()) {
                Ok(BTreeSet::new())
            } else {
                Err(e.into())
            }
        }
    }
}

/// Active protected `record_key`s from `lake.main.record_overlays` (empty if the
/// table isn't there yet — the same missing-table tolerance as suppress). A
/// protected record is pinned master on the read path (PR 1.3b) AND is never-cut
/// by the reconcile never-cut fixpoint (PR 1.3c): no `never` rule may sever its
/// membership. Empty/absent overlay ⇒ reconcile is byte-identical to a run with
/// no protections (the determinism contract holds).
///
/// # Errors
/// Propagates a non-missing-table query error.
fn load_protected_keys(session: &Session) -> Result<BTreeSet<String>> {
    load_overlay_keys(session, "protect")
}

/// S4.5 reconcile: cluster the active auto-merge pairs in `lake.main.match_scores`
/// into connected components, reconcile them against the prior
/// `lake.main.entity_membership` (INV-PERM), and write the resulting
/// membership / `entities` lifecycle / `entity_events` back to the lake.
///
/// NOTE: this is the pure-core path (label_propagate + reconcile_plan) wired to
/// the lake — correct for full-mode base resolution. The relational edge-cut /
/// tombstone event attribution (assertions/deletion) is the next enhancement.
///
/// # Errors
/// Propagates lake DDL / query / write errors.
pub fn run_reconcile(session: &Session, config: &Config, run_id: &str, run_at: &str) -> Result<()> {
    create_ddl_table(session, "entity_membership")?;
    create_ddl_table(session, "entities")?;
    create_ddl_table(session, "entity_events")?;

    let auto_merge = config.thresholds.auto_merge;
    let conn = session.connection();
    // Auto-merge edges carry their probability: never-cut severs the
    // lowest-probability edge on a path, so the weight is load-bearing.
    let mut edges: Vec<PathEdge> =
        er_entities::match_score::read::auto_merge_edges_live(conn, auto_merge)?
            .into_iter()
            .map(|r| (r.rec_a_key, r.rec_b_key, r.match_probability))
            .collect();
    // The node universe is the person or Account standardized corpus, by
    // entity_type; everything else in reconcile (match_scores edges, assertions,
    // membership/entities/events) is shared + entity-agnostic.
    let nodes: Vec<String> = if config.entity_type.is_person() {
        er_entities::int_std_record::read::all_keys_live(conn)?
            .into_iter()
            .map(|r| r.record_key)
            .collect()
    } else {
        er_entities::acct_int_std_record::read::all_keys_live(conn)?
            .into_iter()
            .map(|r| r.record_key)
            .collect()
    };

    // S4.4 assertion adjustment (identical to er-match's proven in-memory path).
    // Empty when the lake has no assertions, so the base run is unchanged.
    let assertions = load_active_assertions(session)?;
    // Suppressed records (PR 1.3 steward overlay): drop them from EVERY
    // clustering input — nodes, match-score edges, and the pushed `always`
    // edges — because label_propagate rebuilds the node universe from the nodes
    // PLUS every edge endpoint, so a surviving edge would resurrect a suppressed
    // node. Suppress therefore wins over `always`. Empty/absent overlay ⇒ the
    // partition is byte-identical to a run with no suppressions.
    let suppressed = load_suppressed_keys(session)?;
    if !suppressed.is_empty() {
        edges.retain(|(a, b, _)| !suppressed.contains(a) && !suppressed.contains(b));
    }
    let nodes: Vec<String> = if suppressed.is_empty() {
        nodes
    } else {
        nodes
            .into_iter()
            .filter(|n| !suppressed.contains(n))
            .collect()
    };
    for a in assertions
        .iter()
        .filter(|a| a.kind == AssertionKind::Always)
    {
        if suppressed.contains(&a.rec_a_key) || suppressed.contains(&a.rec_b_key) {
            continue; // suppress wins over always
        }
        // `always`: a protected edge (prob 1.0 >= cut_protect) never-cut can't sever.
        edges.push((a.rec_a_key.clone(), a.rec_b_key.clone(), 1.0));
    }
    let nevers: Vec<Assertion> = assertions
        .iter()
        .filter(|a| a.kind == AssertionKind::Never)
        .cloned()
        .collect();
    // `never`: drop the direct edge (a bridge splits outright, no partition cut).
    let never_pairs: BTreeSet<(String, String)> = nevers
        .iter()
        .map(|a| (a.rec_a_key.clone(), a.rec_b_key.clone()))
        .collect();
    edges.retain(|(a, b, _)| !never_pairs.contains(&(a.clone(), b.clone())));
    // Protected records (PR 1.3c steward overlay): never-cut may not sever a
    // protected record's membership. Empty/absent overlay ⇒ never-cut is
    // byte-identical to a run with no protections. Loaded once here so the
    // never-cut fixpoint and the escalation surfacing below share the set.
    let protected = load_protected_keys(session)?;
    // never-cut: sever the lowest-probability edge on any path still
    // co-clustering a never pair through OTHER records (S4.4.2). An edge
    // touching a protected record is off-limits; a never pair reachable only
    // through protected records escalates instead (surfaced below).
    if !nevers.is_empty() {
        let result = never_cut_fixpoint(
            &edges,
            &nevers,
            &nodes,
            config.clustering.cut_protect_probability,
            &protected,
            usize::try_from(config.clustering.max_iterations).unwrap_or(50),
        )
        .map_err(|e| anyhow::anyhow!("never-cut: {e:?}"))?;
        let cutset: BTreeSet<(String, String)> = result
            .cuts
            .iter()
            .map(|c| (c.rec_a_key.clone(), c.rec_b_key.clone()))
            .collect();
        edges.retain(|(a, b, _)| !cutset.contains(&(a.clone(), b.clone())));
        // Surface protect-induced contradictions: a never pair that could not be
        // separated without cutting a protected record is a `never_unsatisfiable`
        // review item (the designed reason code, already shown in the review UI).
        // Gated on a protect overlay existing so the review_queue — and the
        // determinism contract — is untouched on the base path. (High-confidence
        // escalations, possible pre-1.3c, stay silent until we rebaseline.)
        if !protected.is_empty() && !result.escalations.is_empty() {
            write_never_unsatisfiable_reviews(session, run_id, run_at, &result.escalations)?;
        }
    }
    let edges: Vec<(String, String)> = edges.into_iter().map(|(a, b, _)| (a, b)).collect();

    // Prior active membership (empty on the first run).
    let mut p_old: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for r in er_entities::entity_membership::read::all_pairs_live(conn)? {
        p_old.entry(r.entity_id).or_default().push(r.record_key);
    }

    let labels = label_propagate(&nodes, &edges);
    let mut by_label: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (record_key, label) in &labels {
        by_label
            .entry(label.clone())
            .or_default()
            .push(record_key.clone());
    }
    let p_new: Vec<Vec<String>> = by_label.into_values().collect();

    // Continue the entity-id counter past every entity ever minted, so a new
    // phase's ids never collide with persisted ones.
    let minted: i64 = er_entities::resolved_entity::read::count_live(conn)?
        .first()
        .and_then(|r| r.n)
        .unwrap_or(0);
    // Entity ids are deterministic UUIDv7s: the run's logical clock + a mint
    // counter continuing past every entity ever minted — rerun-reproducible,
    // collision-free across runs (a later run mints under a later clock).
    let ids = DeterministicV7Factory::from_lake_timestamp(run_at, minted.max(0) as u64)
        .map_err(|e| anyhow::anyhow!("reconcile run_at: {e}"))?;
    let plan = reconcile_plan(&p_old, &p_new, &ids);

    if !plan.assignments.is_empty() {
        let rows: Vec<String> = plan
            .assignments
            .iter()
            .map(|a| {
                let (sys, srid) = a
                    .record_key
                    .split_once(':')
                    .unwrap_or((a.record_key.as_str(), ""));
                format!(
                    "('{}', '{}', '{}', '{}')",
                    esc(sys),
                    esc(srid),
                    esc(&a.record_key),
                    esc(&a.entity_id)
                )
            })
            .collect();
        session.execute_batch(&format!(
            "create temp table __mem_src as select * from (values {}) \
             as t(source_system, source_record_id, record_key, entity_id)",
            rows.join(", ")
        ))?;
        er_entities::entity_membership::update::upsert_from_src(
            session.connection(),
            run_at,
            run_id,
        )?;
        session.execute_batch("drop table __mem_src")?;
    }

    // Membership is upsert-only, so a record that has LEFT the corpus (a
    // tombstone, which standardize drops) keeps its row and still reads as a
    // member of an entity — deletion did not delete. Prune against the
    // standardized corpus rather than against the clustering node set, because
    // the node set also excludes suppressed records, and a suppressed record is
    // still in the corpus and must keep its membership.
    session.execute_batch("drop table if exists __membership_live")?;
    session.execute_batch(&format!(
        "create temp table __membership_live as select record_key from {}",
        if config.entity_type.is_person() {
            INT_STD_RECORDS
        } else {
            ACCT_INT_STD_RECORDS
        }
    ))?;
    er_entities::entity_membership::delete::absent_from_corpus(session.connection())?;
    session.execute_batch("drop table __membership_live")?;

    if !plan.transitions.is_empty() {
        let rows: Vec<String> = plan
            .transitions
            .iter()
            .map(|t| {
                let merged = t
                    .merged_into
                    .as_ref()
                    .map_or_else(|| "null".to_string(), |m| format!("'{}'", esc(m)));
                format!("('{}', '{}', {merged})", esc(&t.entity_id), esc(&t.status))
            })
            .collect();
        session.execute_batch(&format!(
            "create temp table __ent_src as select * from (values {}) \
             as t(entity_id, status, merged_into)",
            rows.join(", ")
        ))?;
        er_entities::resolved_entity::update::upsert_from_src(
            session.connection(),
            run_at,
            run_id,
        )?;
        session.execute_batch("drop table __ent_src")?;
    }

    if !plan.events.is_empty() {
        for (i, e) in plan.events.iter().enumerate() {
            let details = serde_json::to_string(&e.details).unwrap_or_else(|_| "{}".to_string());
            // Content-derived and rerun-stable: the retired per-run counter
            // stream minted DUPLICATE event ids across runs (it restarted at 0).
            let event_id = er_ids::natural_id("entity_events", &[run_id, &i.to_string()]);
            er_entities::entity_event::create::append(
                session.connection(),
                &event_id,
                i64::try_from(i).unwrap_or(i64::MAX),
                run_id,
                &e.entity_id,
                &e.event_type,
                &details,
                &e.details_hash,
                run_at,
            )?;
        }
    }
    Ok(())
}

/// S4.6 golden: assemble `lake.main.golden_lineage` (who won each attribute +
/// the deciding rule) and `lake.main.golden_records` (the surviving values +
/// metadata) from the lake's int_std_records / entity_membership / entities, in
/// full-assembly mode. The generators are er-golden's; here they target lake
/// relations.
///
/// # Errors
/// Propagates the survivorship SQL / lake write errors.
pub fn run_golden(session: &Session, config: &Config, run_at: &str) -> Result<()> {
    if !config.entity_type.is_person() {
        // Account golden assembly over the shared entity tables + the Account
        // standardized corpus. (Steward golden overrides are a person-only path
        // today; not wired for Account.)
        session.execute_batch(&format!("drop table if exists {ACCT_GOLDEN_LINEAGE}"))?;
        let lineage_sql = er_golden::acct_golden_lineage_sql(
            config,
            ACCT_INT_STD_RECORDS,
            ENTITY_MEMBERSHIP,
            ENTITIES,
            run_at,
        )?;
        session.execute_batch(&format!(
            "create table {ACCT_GOLDEN_LINEAGE} as\n{lineage_sql}"
        ))?;

        session.execute_batch(&format!("drop table if exists {ACCT_GOLDEN_RECORDS}"))?;
        let records_sql = er_golden::acct_golden_records_sql(
            config,
            ACCT_GOLDEN_LINEAGE,
            ACCT_INT_STD_RECORDS,
            ENTITY_MEMBERSHIP,
            ENTITIES,
            run_at,
        );
        session.execute_batch(&format!(
            "create table {ACCT_GOLDEN_RECORDS} as\n{records_sql}"
        ))?;
        return Ok(());
    }
    session.execute_batch(&format!("drop table if exists {GOLDEN_LINEAGE}"))?;
    let lineage_sql = er_golden::golden_lineage_sql(
        config,
        INT_STD_RECORDS,
        ENTITY_MEMBERSHIP,
        ENTITIES,
        run_at,
    )?;
    session.execute_batch(&format!("create table {GOLDEN_LINEAGE} as\n{lineage_sql}"))?;

    session.execute_batch(&format!("drop table if exists {GOLDEN_RECORDS}"))?;
    let records_sql = er_golden::golden_records_sql(
        config,
        GOLDEN_LINEAGE,
        INT_STD_RECORDS,
        ENTITY_MEMBERSHIP,
        ENTITIES,
        run_at,
    );
    session.execute_batch(&format!("create table {GOLDEN_RECORDS} as\n{records_sql}"))?;

    // Apply steward golden field overrides (mass-update) — but ONLY if any have
    // been staged. The table is created lazily by the first override action
    // (apply_staged_action), so an org that never used the feature has no
    // golden_overrides table, nothing runs, and golden_records is byte-identical
    // (the determinism contract holds — base fixtures are untouched).
    let overrides_present = session
        .lake_table_exists("golden_overrides")
        .unwrap_or(false);
    if overrides_present {
        // One declared UPDATE per overridable golden column (5 scalar + 6 address).
        use er_entities::golden_record::update as ov;
        let conn = session.connection();
        for apply in [
            ov::override_given_name,
            ov::override_family_name,
            ov::override_email,
            ov::override_phone_e164,
            ov::override_birth_date,
            ov::override_addr_number,
            ov::override_addr_street,
            ov::override_addr_unit,
            ov::override_addr_city,
            ov::override_addr_region,
            ov::override_addr_postal,
        ] {
            apply(conn, run_at)?;
        }
    }
    Ok(())
}

/// Whether a lake query error is "the table isn't there" (vs a real failure):
/// the DuckDB/DuckLake surface for a relation that the pipeline/drain hasn't
/// created yet. Lets readers treat an absent relation as empty.
fn is_missing_table(err: &str) -> bool {
    err.contains("does not exist")
        || err.contains("Catalog Error")
        || err.contains("Table with name")
}

/// The lake relations `doctor` inspects, by bare name, in pipeline order. The
/// DuckLake-resident outputs the Rust engine writes (the control-plane tables
/// live in Postgres, not here). Presence/row counts come from the Session's
/// catalog-introspection helpers; the names stay covered by the declared-names
/// gate.
const DOCTOR_RELATIONS: [&str; 12] = [
    "raw_records",
    "int_std_records",
    "match_scores",
    "entity_membership",
    "entities",
    "entity_events",
    "golden_records",
    "golden_lineage",
    "runs",
    "review_queue",
    "assertions",
    "ingest_batches",
];

/// A read-only health report over the lake + config (`er doctor`): the config
/// fingerprint + versions, each lake relation's presence and row count, and the
/// latest run's status. `ok` is true once the core resolved relations
/// (`int_std_records`, `entity_membership`, `golden_records`) are all present —
/// i.e. the pipeline has produced output — so a fresh, unpopulated lake reports
/// `ok: false` with every relation absent rather than erroring.
///
/// # Errors
/// Propagates a non-missing-table lake query error (a genuinely broken lake).
pub fn doctor(session: &Session, config: &Config) -> Result<serde_json::Value> {
    let mut relations = Vec::with_capacity(DOCTOR_RELATIONS.len());
    let mut present: std::collections::BTreeMap<&str, bool> = std::collections::BTreeMap::new();
    for name in DOCTOR_RELATIONS {
        let count = session.relation_row_count(name)?;
        present.insert(name, count.is_some());
        relations.push(serde_json::json!({
            "name": name,
            "present": count.is_some(),
            "rows": count.unwrap_or(0),
        }));
    }
    let ok = ["int_std_records", "entity_membership", "golden_records"]
        .iter()
        .all(|n| present.get(n).copied().unwrap_or(false));

    // The latest run (if the ledger exists): run_id + status + mode.
    let latest_run = match er_entities::run::read::latest_live(session.connection()) {
        Ok(rows) => rows
            .first()
            .map(|r| serde_json::json!({ "run_id": r.id, "status": r.status, "mode": r.mode })),
        Err(e) if is_missing_table(&e.to_string()) => None,
        Err(e) => return Err(e.into()),
    };

    Ok(serde_json::json!({
        "tenant": config.tenant,
        "config_hash": er_config::config_hash(config),
        "std_version": config.versions.std_version,
        "survivorship_version": config.versions.survivorship_version,
        "relations": relations,
        "latest_run": latest_run,
        "ok": ok,
    }))
}

/// Per-table `(file_count, delete_file_count)` as a JSON array.
fn stats_json(stats: &[(String, i64, i64)]) -> serde_json::Value {
    serde_json::Value::Array(
        stats
            .iter()
            .map(|(table, files, deletes)| {
                serde_json::json!({
                    "table": table,
                    "file_count": files,
                    "delete_file_count": deletes,
                })
            })
            .collect(),
    )
}

/// Lake maintenance (`er maintain`): flush inlined (catalog-resident) rows out to
/// Parquet, merge small adjacent data files, and expire snapshots older than
/// `expire_older_than` (a DuckDB interval string, e.g. `"7 days"` — keep it well
/// above the longest-running read). Returns the before/after per-table file
/// counts so an operator (or `GET /runs`) can see the compaction effect. This is
/// the data-plane op behind the `lake_maintain` schedulable job kind.
///
/// # Errors
/// Propagates the DuckLake maintenance call errors.
pub fn run_maintain(session: &Session, expire_older_than: &str) -> Result<serde_json::Value> {
    let before = session.lake_table_stats()?;
    session.flush_inlined_data()?;
    session.merge_adjacent_files(None, None)?;
    session.expire_snapshots(expire_older_than)?;
    let after = session.lake_table_stats()?;
    Ok(serde_json::json!({
        "expired_older_than": expire_older_than,
        "before": stats_json(&before),
        "after": stats_json(&after),
    }))
}

/// Everything a full-mode `run-all` needs beyond the config + lake session: the
/// lexicon seed, the per-source deliveries, the committed model, and the run's
/// id + volatile stamps.
pub struct PipelineInputs<'a> {
    pub lexicon: &'a [(String, String)],
    pub deliveries: &'a [(String, String)],
    pub model: &'a MatchModel<'a>,
    pub run_id: &'a str,
    pub ingested_at: &'a str,
    pub scored_at: &'a str,
    pub run_at: &'a str,
    /// The mode this run is actually executing (`full` / `incremental`), written
    /// to the ledger. Defaults to `full` via [`PipelineInputs::full`].
    pub mode: &'a str,
    /// Why this run is rebuilding, when it is not the ordinary case: an
    /// incremental run promoted by the drift guard records the drift here.
    pub rebuild_reason: Option<&'a str>,
    /// A `config.segments` name to restrict the match pass to (#54). The caller
    /// resolves the name against the published config and passes the predicate,
    /// so no SQL arrives from a request. Implies an incremental-shaped pass:
    /// restricting a FULL pass to a slice would be the destructive reading of
    /// scoping (see [`run_match_segment`]).
    pub segment: Option<(&'a str, &'a str)>,
}

impl<'a> PipelineInputs<'a> {
    /// The ordinary full-mode inputs: mode `full`, no rebuild reason.
    #[must_use]
    pub fn full(
        lexicon: &'a [(String, String)],
        deliveries: &'a [(String, String)],
        model: &'a MatchModel<'a>,
        run_id: &'a str,
        stamp: &'a str,
    ) -> Self {
        PipelineInputs {
            lexicon,
            deliveries,
            model,
            run_id,
            ingested_at: stamp,
            scored_at: stamp,
            run_at: stamp,
            mode: er_versions::MODE_FULL,
            rebuild_reason: None,
            segment: None,
        }
    }
}

/// The S4.0 run chain over the lake: seed lexicon -> ingest every delivery ->
/// standardize -> match -> reconcile -> golden, under one run id. The single
/// entry point the `er run-all` binary and the control-plane dispatcher share.
///
/// `inputs.mode` now decides which match pass runs, where before it only
/// labelled the ledger row: `incremental` scores the pairs touching new, changed
/// and departed records (S4.3.4) instead of re-scoring the corpus. Everything
/// downstream of match is unchanged and deliberately so — see
/// [`run_match_incremental`] for why that is equal to a full pass rather than
/// cheaper-but-approximate, and the note below for why reconcile stays whole.
///
/// # Errors
/// Propagates any stage error.
pub fn run_all(session: &Session, config: &Config, inputs: &PipelineInputs) -> Result<()> {
    write_run_start(session, config, inputs)?;
    seed_lexicon(session, inputs.lexicon)?;
    for (source, csv) in inputs.deliveries {
        run_ingest(
            session,
            config,
            source,
            csv,
            inputs.run_id,
            inputs.ingested_at,
            false,
        )?;
        write_ingest_batch(
            session,
            inputs.run_id,
            source,
            source,
            false,
            inputs.ingested_at,
        )?;
    }
    run_standardize(session, config, inputs.run_at)?;
    if inputs.mode == er_versions::MODE_INCREMENTAL || inputs.segment.is_some() {
        let delta = run_match_partial(
            session,
            config,
            inputs.model,
            config.thresholds.review_low,
            inputs.run_id,
            inputs.scored_at,
            inputs.segment.map(|(_, predicate)| predicate),
        )?;
        eprintln!(
            "run {}: {} match over {}/{} records, {} scores invalidated{}{}",
            inputs.run_id,
            delta.mode,
            delta.delta_records,
            delta.corpus_records,
            delta.invalidated_scores,
            inputs
                .segment
                .map(|(name, _)| format!(", segment {name}"))
                .unwrap_or_default(),
            delta
                .full_reason
                .map(|r| format!(" ({r})"))
                .unwrap_or_default(),
        );
    } else {
        run_match(
            session,
            config,
            inputs.model,
            config.thresholds.review_low,
            inputs.run_id,
            inputs.scored_at,
        )?;
    }
    // Reconcile and golden stay whole-corpus in both modes. That is a choice,
    // not an omission: clustering is a function of the edge set, and connected
    // components over the whole (small) above-threshold graph is linear in edges
    // and exactly reproducible, while the cost incremental mode exists to avoid
    // is the QUADRATIC pair generation in match. Re-clustering from the full
    // edge set also means the partition cannot drift from what a full run would
    // produce, which is the property the equivalence gate asserts.
    run_reconcile(session, config, inputs.run_id, inputs.run_at)?;
    run_golden(session, config, inputs.run_at)?;
    write_reviews(session, config, inputs.run_id, inputs.run_at)?;
    write_run_end(session, inputs.run_id, inputs.run_at)?;
    Ok(())
}

/// The "apply staged corrections" pass (`er correct`; the data-plane op behind a
/// `run_all_incremental { skip_ingest: true }` job): re-run reconcile
/// (assertion-adjusted) → golden → reviews over the EXISTING `match_scores`, so
/// drained assertions / unmerges take effect WITHOUT re-ingesting or re-scoring.
/// The `runs` row is mode `correct`, reusing the model identity of the latest
/// run (the corpus was scored against it).
///
/// # Errors
/// Returns an error if there is no prior run to correct (nothing has been scored
/// yet); otherwise propagates the reconcile / golden / lake errors.
pub fn run_correct(session: &Session, config: &Config, run_id: &str, run_at: &str) -> Result<()> {
    let latest = er_entities::run::read::latest_live(session.connection()).map_err(|e| {
        if is_missing_table(&e.to_string()) {
            anyhow::anyhow!("no prior run to correct; run a full run first")
        } else {
            e.into()
        }
    })?;
    // A trained run always stamps both; an untrained prior run corrects with
    // empty identity rather than the old stringified-NULL literal.
    let (model_version, tf_snapshot_id) = latest
        .into_iter()
        .next()
        .map(|r| {
            (
                r.model_version.unwrap_or_default(),
                r.tf_snapshot_id.unwrap_or_default(),
            )
        })
        .ok_or_else(|| anyhow::anyhow!("no prior run to correct; run a full run first"))?;

    write_run_row(
        session,
        config,
        run_id,
        run_at,
        "correct",
        &model_version,
        &tf_snapshot_id,
        None,
        Some(er_versions::REBUILD_CORRECTION_PASS),
    )?;
    run_reconcile(session, config, run_id, run_at)?;
    run_golden(session, config, run_at)?;
    write_reviews(session, config, run_id, run_at)?;
    write_run_end(session, run_id, run_at)?;
    Ok(())
}

/// What `run_train` needs: the base settings TEMPLATE (comparison + blocking
/// structure — training fits m/u/prior onto it), the training blocking rules
/// (deterministic for the prior, EM for the sessions), and the identity to stamp
/// the exported model + TF snapshot with.
pub struct TrainInputs<'a> {
    pub template_settings_json: &'a str,
    pub deterministic_rules: &'a [String],
    pub em_rules: &'a [String],
    pub recall: f64,
    pub u_max_pairs: f64,
    pub model_version: &'a str,
    pub tf_snapshot_id: &'a str,
}

/// Export the frozen term-frequency snapshot the trained model scores against
/// (`model_version, tf_snapshot_id, column_name, value, tf_value`) — the CSV
/// shape [`build_frozen_tf_tables`](crate::run_match) reads back. One row per
/// (TF column, value); a model with no TF columns writes just the header. Uses
/// DuckDB `COPY` so value quoting is the engine's problem, not ours.
fn export_tf_snapshot(
    session: &Session,
    tf_columns: &[String],
    tf_prefix: &str,
    handles: &[dedupe_core::session::TableHandle],
    model_version: &str,
    tf_snapshot_id: &str,
    out_tf: &Path,
) -> Result<()> {
    let path = out_tf.to_string_lossy();
    if tf_columns.is_empty() {
        std::fs::write(
            out_tf,
            "model_version,tf_snapshot_id,column_name,value,tf_value\n",
        )?;
        return Ok(());
    }
    let mut selects = Vec::with_capacity(tf_columns.len());
    for col in tf_columns {
        let handle = handles
            .iter()
            .find(|h| h.columns.iter().any(|c| c == col))
            .ok_or_else(|| anyhow::anyhow!("no TF table materialized for column {col:?}"))?;
        selects.push(format!(
            "select '{mv}' as model_version, '{snap}' as tf_snapshot_id, '{col}' as column_name, \
             cast(\"{col}\" as varchar) as value, cast(\"{tfp}{col}\" as varchar) as tf_value \
             from {phys}",
            mv = esc(model_version),
            snap = esc(tf_snapshot_id),
            col = esc(col),
            tfp = esc(tf_prefix),
            phys = handle.physical,
        ));
    }
    session.execute_batch(&format!(
        "COPY ({}) TO '{}' (HEADER, DELIMITER ',')",
        selects.join("\nUNION ALL\n"),
        esc(&path),
    ))?;
    Ok(())
}

/// S4.3.2 training (`er train`): fit a match model on the lake's
/// `int_std_records` and write BOTH artifacts a later `full` match freezes
/// against — the trained settings JSON (`out_model`) and the TF snapshot CSV
/// (`out_tf`), stamped with `model_version` / `tf_snapshot_id`. The comparison +
/// blocking STRUCTURE comes from the template; EM fits the parameters (the EM
/// estimator itself is dedupe-core's, bit-parity-proven). Registering the model
/// in a registry and promoting it is the control-plane's job; this is the
/// data-plane step.
///
/// # Errors
/// Propagates the model-load / linker / training / export / save errors.
pub fn run_train(
    session: &Session,
    inputs: &TrainInputs,
    out_model: &Path,
    out_tf: &Path,
) -> Result<()> {
    let settings = er_match::load_model(inputs.template_settings_json)?;
    let input = session.register_lake_table("int_std_records", "int_std_records")?;
    let mut linker = dedupe_core::linker::Linker::new(session, settings, &input)
        .map_err(|e| anyhow::anyhow!("linker setup: {e}"))?;
    linker
        .train(&dedupe_core::linker::TrainConfig {
            deterministic_rules: inputs.deterministic_rules.to_vec(),
            recall: inputs.recall,
            u_max_pairs: inputs.u_max_pairs,
            em_rules: inputs.em_rules.to_vec(),
        })
        .map_err(|e| anyhow::anyhow!("train: {e}"))?;

    // Export the TF snapshot BEFORE consuming the linker for the model.
    let tf_columns = linker.settings_ref().tf_columns();
    let tf_prefix = linker.settings_ref().tf_prefix.clone();
    export_tf_snapshot(
        session,
        &tf_columns,
        &tf_prefix,
        linker.tf_tables_ref(),
        inputs.model_version,
        inputs.tf_snapshot_id,
        out_tf,
    )?;

    linker
        .into_model()
        .save_to_json_file(out_model)
        .map_err(|e| anyhow::anyhow!("save trained model: {e}"))?;
    Ok(())
}
