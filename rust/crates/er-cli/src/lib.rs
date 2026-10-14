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
//! Splink `__splink__*` intermediates stay in the native temp catalog;
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
use er_ingest::landing::{create_raw_records_sql, land_base_delivery, land_full_refresh};
use std::collections::BTreeMap;

use er_ids::{CountingIdFactory, IdFactory};
use er_match::{build_frozen_tf_tables, load_model};
use er_reconcile::assertions::{Assertion, AssertionKind};
use er_reconcile::cluster::label_propagate;
use er_reconcile::never_cut::{PathEdge, never_cut_fixpoint};
use er_reconcile::reconcile::reconcile_plan;
use er_standardize::build_int_std_records;
use std::collections::BTreeSet;

/// The lake relations this crate reads/writes, fully qualified.
pub const RAW_RECORDS: &str = "lake.main.raw_records";
pub const INT_STD_RECORDS: &str = "lake.main.int_std_records";
pub const MATCH_SCORES: &str = "lake.main.match_scores";
pub const ENTITY_MEMBERSHIP: &str = "lake.main.entity_membership";
pub const ENTITIES: &str = "lake.main.entities";
pub const ENTITY_EVENTS: &str = "lake.main.entity_events";
pub const GOLDEN_LINEAGE: &str = "lake.main.golden_lineage";
pub const GOLDEN_RECORDS: &str = "lake.main.golden_records";
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
    // `create table if not exists` so repeated deliveries append to one relation.
    let ddl = create_raw_records_sql(RAW_RECORDS).replacen(
        "create table",
        "create table if not exists",
        1,
    );
    session.execute_batch(&ddl)?;
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
        RAW_RECORDS,
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
pub fn run_standardize(session: &Session, config: &Config) -> Result<()> {
    session.execute_batch(&format!("drop table if exists {INT_STD_RECORDS}"))?;
    build_int_std_records(session, config, RAW_RECORDS, LEXICON, INT_STD_RECORDS)
}

/// The committed model a `full` match scores against (S4.3.2): the frozen Splink
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
    model: &MatchModel,
    review_low: f64,
    run_id: &str,
    scored_at: &str,
) -> Result<()> {
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

    let spec = er_schema::model::all_specs()
        .into_iter()
        .find(|s| s.name == "match_scores")
        .ok_or_else(|| anyhow::anyhow!("match_scores spec missing from S5 registry"))?;
    session.execute_batch(
        &er_schema::model::create_table_sql(&spec).map_err(|e| anyhow::anyhow!("{e}"))?,
    )?;

    // Canonicalise, keep the highest-probability row per pair, threshold at
    // review_low, join both endpoint content hashes, and render the evidence —
    // exactly er.matching.full's _selected_predictions_sql + _source_sql.
    let uid = &settings.unique_id_column;
    let evidence = evidence_json(&settings);
    let run_id = run_id.replace('\'', "''");
    let scored_at = scored_at.replace('\'', "''");
    let (mv, snap) = (
        model.model_version.replace('\'', "''"),
        model.tf_snapshot_id.replace('\'', "''"),
    );
    session.execute_batch(&format!(
        "create temp table __match_src as
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
         select s.rec_a_key, s.rec_b_key, s.match_probability,
                '{mv}' as model_version, '{snap}' as tf_snapshot_id,
                ra.content_hash as rec_a_content_hash, rb.content_hash as rec_b_content_hash,
                s.evidence, true as is_active,
                cast(null as timestamp) as invalidated_at, cast(null as varchar) as invalidated_run_id,
                '{run_id}' as run_id, cast('{scored_at}' as timestamp) as scored_at
         from scored s
         join {INT_STD_RECORDS} ra on ra.record_key = s.rec_a_key
         join {INT_STD_RECORDS} rb on rb.record_key = s.rec_b_key",
        preds = predictions.physical,
    ))?;

    // S4.3.4 single write: MERGE on the logical key. A re-scored pair is
    // rewritten in place (clearing any invalidation); a new pair is inserted.
    session.execute_batch(&format!(
        "merge into {MATCH_SCORES} as target using __match_src as source
           on target.rec_a_key = source.rec_a_key and target.rec_b_key = source.rec_b_key
          and target.model_version = source.model_version
          and target.tf_snapshot_id = source.tf_snapshot_id
         when matched then update set
             match_probability = source.match_probability,
             rec_a_content_hash = source.rec_a_content_hash,
             rec_b_content_hash = source.rec_b_content_hash,
             evidence = source.evidence,
             is_active = true,
             invalidated_at = null,
             invalidated_run_id = null,
             run_id = source.run_id,
             scored_at = source.scored_at
         when not matched then insert
             (rec_a_key, rec_b_key, match_probability, model_version, tf_snapshot_id,
              rec_a_content_hash, rec_b_content_hash, evidence, is_active,
              invalidated_at, invalidated_run_id, run_id, scored_at)
             values
             (source.rec_a_key, source.rec_b_key, source.match_probability, source.model_version,
              source.tf_snapshot_id, source.rec_a_content_hash, source.rec_b_content_hash,
              source.evidence, true, null, null, source.run_id, source.scored_at)"
    ))?;
    session.execute_batch("drop table __match_src")?;
    Ok(())
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

/// Record the `runs` row at the start of a full run (status `running`): the run
/// ledger the read API's `GET /runs` surfaces. `started_at` is the run's logical
/// timestamp; `code_version` is this crate's version.
///
/// # Errors
/// Propagates lake DDL / insert errors.
pub fn write_run_start(session: &Session, config: &Config, inputs: &PipelineInputs) -> Result<()> {
    write_run_row(
        session,
        config,
        inputs.run_id,
        inputs.run_at,
        "full",
        inputs.model.model_version,
        inputs.model.tf_snapshot_id,
    )
}

/// Insert one `running` row into the `runs` ledger with the given `mode` and the
/// model identity the run scored against. `code_version` is this crate's version.
fn write_run_row(
    session: &Session,
    config: &Config,
    run_id: &str,
    started_at: &str,
    mode: &str,
    model_version: &str,
    tf_snapshot_id: &str,
) -> Result<()> {
    create_ddl_table(session, "runs")?;
    session.execute_batch(&format!(
        "INSERT INTO {RUNS} (run_id, tenant, mode, status, started_at, config_hash, \
           model_version, tf_snapshot_id, std_version, survivorship_version, code_version) \
         VALUES ('{run_id}', '{tenant}', '{mode}', 'running', '{started_at}', '{hash}', \
           '{model_version}', '{tf_snapshot_id}', '{std_v}', '{surv_v}', '{code_v}')",
        run_id = esc(run_id),
        tenant = esc(&config.tenant),
        mode = esc(mode),
        started_at = esc(started_at),
        hash = esc(&er_config::config_hash(config)),
        model_version = esc(model_version),
        tf_snapshot_id = esc(tf_snapshot_id),
        std_v = esc(&config.versions.std_version),
        surv_v = esc(&config.versions.survivorship_version),
        code_v = esc(env!("CARGO_PKG_VERSION")),
    ))?;
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
    let r = er_ingest::landing::batch_receipt(session, RAW_RECORDS, source, run_id)?;
    create_ddl_table(session, "ingest_batches")?;
    session.execute_batch(&format!(
        "INSERT INTO {INGEST_BATCHES} \
           (ingest_batch_id, run_id, source_system, path, new_count, changed_count, \
            unchanged_count, tombstone_count, resurrected_count, full_refresh_keys, created_at) \
         VALUES ('{id}', '{rid}', '{src}', '{path}', {new}, {chg}, {unch}, {tomb}, {res}, {fr}, '{at}')",
        id = esc(&format!("{run_id}:{source}")),
        rid = esc(run_id),
        src = esc(source),
        path = esc(path),
        new = r.new,
        chg = r.changed,
        unch = r.unchanged,
        tomb = r.tombstone,
        res = r.resurrected,
        fr = full_refresh,
        at = esc(created_at),
    ))?;
    Ok(())
}

/// Close the `runs` row on success (status `succeeded`, `ended_at` stamped).
///
/// # Errors
/// Propagates the lake update error.
pub fn write_run_end(session: &Session, run_id: &str, ended_at: &str) -> Result<()> {
    session.execute_batch(&format!(
        "UPDATE {RUNS} SET status = 'succeeded', ended_at = '{}' WHERE run_id = '{}'",
        esc(ended_at),
        esc(run_id),
    ))?;
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
            let note = action
                .get("note")
                .and_then(serde_json::Value::as_str)
                .map_or("NULL".to_string(), |n| format!("'{}'", esc(n)));
            session.execute_batch(&format!(
                "INSERT INTO {ASSERTIONS} \
                   (assertion_id, rec_a_key, rec_b_key, kind, active, created_by, created_at, note) \
                 VALUES ('{id}', '{lo}', '{hi}', '{kind}', true, '{by}', '{at}', {note})",
                id = esc(action_id),
                lo = esc(lo),
                hi = esc(hi),
                kind = esc(field("kind")),
                by = esc(field("created_by")),
                at = esc(applied_at),
            ))?;
        }
        Some("retract_assertion") => {
            create_ddl_table(session, "assertions")?;
            session.execute_batch(&format!(
                "UPDATE {ASSERTIONS} SET active = false, retracted_at = '{at}' \
                 WHERE assertion_id = '{id}'",
                at = esc(applied_at),
                id = esc(field("assertion_id")),
            ))?;
        }
        Some("resolve_review") => {
            let rid = esc(field("review_id"));
            let resolution = field("resolution");
            let status = match resolution {
                "match" => "resolved_match",
                "no_match" => "resolved_no_match",
                _ => "dismissed",
            };
            session.execute_batch(&format!(
                "UPDATE {REVIEW_QUEUE} SET status = '{status}', resolved_at = '{at}' \
                 WHERE review_id = '{rid}'",
                at = esc(applied_at),
            ))?;
            // match/no_match leave an always/never assertion on the review's pair.
            if let Some(kind) = match resolution {
                "match" => Some("always"),
                "no_match" => Some("never"),
                _ => None,
            } {
                create_ddl_table(session, "assertions")?;
                session.execute_batch(&format!(
                    "INSERT INTO {ASSERTIONS} \
                       (assertion_id, rec_a_key, rec_b_key, kind, active, created_by, created_at) \
                     SELECT '{id}', rec_a_key, rec_b_key, '{kind}', true, 'steward', '{at}' \
                     FROM {REVIEW_QUEUE} WHERE review_id = '{rid}'",
                    id = esc(action_id),
                    at = esc(applied_at),
                ))?;
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
pub fn write_reviews(session: &Session, config: &Config, run_id: &str) -> Result<()> {
    create_ddl_table(session, "review_queue")?;
    let run_id = esc(run_id);
    session.execute_batch(&format!(
        "create temp table __review_src as \
         select 'gray_band:' || rec_a_key || ':' || rec_b_key as review_id, \
                'pair' as subject_type, rec_a_key, rec_b_key, \
                cast(null as varchar) as entity_id, 'gray_band' as reason, \
                match_probability, evidence as waterfall, 'open' as status, \
                '{run_id}' as first_seen_run_id, '{run_id}' as last_seen_run_id, \
                cast(null as varchar) as resolved_by, cast(null as timestamp) as resolved_at \
         from {MATCH_SCORES} \
         where is_active and match_probability >= {low} and match_probability < {high}",
        low = config.thresholds.review_low,
        high = config.thresholds.auto_merge,
    ))?;
    session.execute_batch(&format!(
        "merge into {REVIEW_QUEUE} as target using __review_src as source \
           on target.rec_a_key = source.rec_a_key and target.rec_b_key = source.rec_b_key \
          and target.reason = source.reason \
         when matched then update set match_probability = source.match_probability, \
             waterfall = source.waterfall, last_seen_run_id = source.last_seen_run_id \
         when not matched then insert \
             (review_id, subject_type, rec_a_key, rec_b_key, entity_id, reason, \
              match_probability, waterfall, status, first_seen_run_id, last_seen_run_id, \
              resolved_by, resolved_at) \
             values \
             (source.review_id, source.subject_type, source.rec_a_key, source.rec_b_key, \
              source.entity_id, source.reason, source.match_probability, source.waterfall, \
              source.status, source.first_seen_run_id, source.last_seen_run_id, \
              source.resolved_by, source.resolved_at)"
    ))?;
    session.execute_batch("drop table __review_src")?;
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
    let rows = match session.query_strings(&format!(
        "select assertion_id, rec_a_key, rec_b_key, kind from {ASSERTIONS} where active"
    )) {
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
            let kind = match r[3].as_str() {
                "always" => AssertionKind::Always,
                "never" => AssertionKind::Never,
                _ => return None,
            };
            let (a, b) = canon(&r[1], &r[2]);
            Some(Assertion {
                assertion_id: r[0].clone(),
                rec_a_key: a,
                rec_b_key: b,
                kind,
                active: true,
            })
        })
        .collect())
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
    // Auto-merge edges carry their probability: never-cut severs the
    // lowest-probability edge on a path, so the weight is load-bearing.
    let mut edges: Vec<PathEdge> = session
        .query_strings(&format!(
            "select rec_a_key, rec_b_key, match_probability from {MATCH_SCORES} \
             where is_active and match_probability >= {auto_merge} order by rec_a_key, rec_b_key"
        ))?
        .into_iter()
        .map(|r| {
            (
                r[0].clone(),
                r[1].clone(),
                r[2].parse::<f64>().unwrap_or(1.0),
            )
        })
        .collect();
    let nodes: Vec<String> = session
        .query_strings(&format!(
            "select record_key from {INT_STD_RECORDS} order by record_key"
        ))?
        .into_iter()
        .map(|r| r[0].clone())
        .collect();

    // S4.4 assertion adjustment (identical to er-match's proven in-memory path).
    // Empty when the lake has no assertions, so the base run is unchanged.
    let assertions = load_active_assertions(session)?;
    for a in assertions
        .iter()
        .filter(|a| a.kind == AssertionKind::Always)
    {
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
    // never-cut: sever the lowest-probability edge on any path still
    // co-clustering a never pair through OTHER records (S4.4.2).
    if !nevers.is_empty() {
        let cuts = never_cut_fixpoint(
            &edges,
            &nevers,
            &nodes,
            config.clustering.cut_protect_probability,
            usize::try_from(config.clustering.max_iterations).unwrap_or(50),
        )
        .map_err(|e| anyhow::anyhow!("never-cut: {e:?}"))?
        .cuts;
        let cutset: BTreeSet<(String, String)> = cuts
            .iter()
            .map(|c| (c.rec_a_key.clone(), c.rec_b_key.clone()))
            .collect();
        edges.retain(|(a, b, _)| !cutset.contains(&(a.clone(), b.clone())));
    }
    let edges: Vec<(String, String)> = edges.into_iter().map(|(a, b, _)| (a, b)).collect();

    // Prior active membership (empty on the first run).
    let mut p_old: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for r in session.query_strings(&format!(
        "select entity_id, record_key from {ENTITY_MEMBERSHIP} order by entity_id, record_key"
    ))? {
        p_old.entry(r[0].clone()).or_default().push(r[1].clone());
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
    let minted: i64 = session.query_strings(&format!("select count(*) from {ENTITIES}"))?[0][0]
        .parse()
        .unwrap_or(0);
    let plan = reconcile_plan(
        &p_old,
        &p_new,
        &CountingIdFactory::new(minted.max(0) as u128),
    );

    let ts = esc(run_at);
    let rid = esc(run_id);

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
        session.execute_batch(&format!(
            "merge into {ENTITY_MEMBERSHIP} as target using __mem_src as source
               on target.source_system = source.source_system
              and target.source_record_id = source.source_record_id
             when matched then update set record_key = source.record_key,
                 entity_id = source.entity_id, assigned_at = cast('{ts}' as timestamp), run_id = '{rid}'
             when not matched then insert
                 (source_system, source_record_id, record_key, entity_id, assigned_at, run_id)
                 values (source.source_system, source.source_record_id, source.record_key,
                         source.entity_id, cast('{ts}' as timestamp), '{rid}')"
        ))?;
        session.execute_batch("drop table __mem_src")?;
    }

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
        session.execute_batch(&format!(
            "merge into {ENTITIES} as target using __ent_src as source
               on target.entity_id = source.entity_id
             when matched then update set status = source.status,
                 merged_into = source.merged_into, updated_at = cast('{ts}' as timestamp),
                 updated_run_id = '{rid}'
             when not matched then insert
                 (entity_id, status, merged_into, created_at, updated_at, created_run_id, updated_run_id)
                 values (source.entity_id, source.status, source.merged_into,
                         cast('{ts}' as timestamp), cast('{ts}' as timestamp), '{rid}', '{rid}')"
        ))?;
        session.execute_batch("drop table __ent_src")?;
    }

    if !plan.events.is_empty() {
        let event_ids = CountingIdFactory::new(0);
        let rows: Vec<String> = plan
            .events
            .iter()
            .enumerate()
            .map(|(i, e)| {
                let details =
                    serde_json::to_string(&e.details).unwrap_or_else(|_| "{}".to_string());
                format!(
                    "('{}', {i}, '{rid}', '{}', '{}', '{}'::json, '{}', cast('{ts}' as timestamp))",
                    esc(&event_ids.new_id()),
                    esc(&e.entity_id),
                    esc(&e.event_type),
                    esc(&details),
                    esc(&e.details_hash),
                )
            })
            .collect();
        session.execute_batch(&format!(
            "insert into {ENTITY_EVENTS} \
             (event_id, seq, run_id, entity_id, event_type, details, details_hash, occurred_at) \
             values {}",
            rows.join(", ")
        ))?;
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
pub fn run_golden(session: &Session, config: &Config) -> Result<()> {
    session.execute_batch(&format!("drop table if exists {GOLDEN_LINEAGE}"))?;
    let lineage_sql =
        er_golden::golden_lineage_sql(config, INT_STD_RECORDS, ENTITY_MEMBERSHIP, ENTITIES)?;
    session.execute_batch(&format!("create table {GOLDEN_LINEAGE} as\n{lineage_sql}"))?;

    session.execute_batch(&format!("drop table if exists {GOLDEN_RECORDS}"))?;
    let records_sql = er_golden::golden_records_sql(
        config,
        GOLDEN_LINEAGE,
        INT_STD_RECORDS,
        ENTITY_MEMBERSHIP,
        ENTITIES,
    );
    session.execute_batch(&format!("create table {GOLDEN_RECORDS} as\n{records_sql}"))?;
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

/// `(present, row_count)` for a fully-qualified lake relation — `(false, 0)` when
/// the relation hasn't been created yet.
fn relation_count(session: &Session, qualified: &str) -> Result<(bool, i64)> {
    match session.query_strings(&format!("select count(*) from {qualified}")) {
        Ok(rows) => {
            let n = rows
                .first()
                .and_then(|r| r.first())
                .map_or(0, |v| v.parse::<i64>().unwrap_or(0));
            Ok((true, n))
        }
        Err(e) => {
            if is_missing_table(&e.to_string()) {
                Ok((false, 0))
            } else {
                Err(e.into())
            }
        }
    }
}

/// The lake relations `doctor` inspects, by bare name -> fully-qualified, in
/// pipeline order. The DuckLake-resident outputs the Rust engine writes (the
/// control-plane tables live in Postgres, not here).
const DOCTOR_RELATIONS: [(&str, &str); 12] = [
    ("raw_records", RAW_RECORDS),
    ("int_std_records", INT_STD_RECORDS),
    ("match_scores", MATCH_SCORES),
    ("entity_membership", ENTITY_MEMBERSHIP),
    ("entities", ENTITIES),
    ("entity_events", ENTITY_EVENTS),
    ("golden_records", GOLDEN_RECORDS),
    ("golden_lineage", GOLDEN_LINEAGE),
    ("runs", RUNS),
    ("review_queue", REVIEW_QUEUE),
    ("assertions", ASSERTIONS),
    ("ingest_batches", INGEST_BATCHES),
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
    for (name, qualified) in DOCTOR_RELATIONS {
        let (is_present, rows) = relation_count(session, qualified)?;
        present.insert(name, is_present);
        relations.push(serde_json::json!({ "name": name, "present": is_present, "rows": rows }));
    }
    let ok = ["int_std_records", "entity_membership", "golden_records"]
        .iter()
        .all(|n| present.get(n).copied().unwrap_or(false));

    // The latest run (if the ledger exists): run_id + status + mode.
    let latest_run = match session.query_strings(&format!(
        "select run_id, status, mode from {RUNS} order by started_at desc limit 1"
    )) {
        Ok(rows) => rows
            .first()
            .map(|r| serde_json::json!({ "run_id": r[0], "status": r[1], "mode": r[2] })),
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
}

/// The S4.0 full-mode chain over the lake: seed lexicon -> ingest every delivery
/// -> standardize -> match -> reconcile -> golden, under one run id. The single
/// entry point the `er run-all` binary and the control-plane dispatcher share.
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
    run_standardize(session, config)?;
    run_match(
        session,
        inputs.model,
        config.thresholds.review_low,
        inputs.run_id,
        inputs.scored_at,
    )?;
    run_reconcile(session, config, inputs.run_id, inputs.run_at)?;
    run_golden(session, config)?;
    write_reviews(session, config, inputs.run_id)?;
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
    let latest = session
        .query_strings(&format!(
            "select model_version, tf_snapshot_id from {RUNS} \
             order by started_at desc, run_id desc limit 1"
        ))
        .map_err(|e| {
            if is_missing_table(&e.to_string()) {
                anyhow::anyhow!("no prior run to correct; run a full run first")
            } else {
                e.into()
            }
        })?;
    let (model_version, tf_snapshot_id) = latest
        .first()
        .map(|r| (r[0].clone(), r[1].clone()))
        .ok_or_else(|| anyhow::anyhow!("no prior run to correct; run a full run first"))?;

    write_run_row(
        session,
        config,
        run_id,
        run_at,
        "correct",
        &model_version,
        &tf_snapshot_id,
    )?;
    run_reconcile(session, config, run_id, run_at)?;
    run_golden(session, config)?;
    write_reviews(session, config, run_id)?;
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

/// S4.3.2 training (`er train`): fit a Splink model on the lake's
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
