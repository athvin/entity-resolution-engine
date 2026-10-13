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
use er_reconcile::cluster::label_propagate;
use er_reconcile::reconcile::reconcile_plan;
use er_standardize::build_int_std_records;

/// The lake relations this crate reads/writes, fully qualified.
pub const RAW_RECORDS: &str = "lake.main.raw_records";
pub const INT_STD_RECORDS: &str = "lake.main.int_std_records";
pub const MATCH_SCORES: &str = "lake.main.match_scores";
pub const ENTITY_MEMBERSHIP: &str = "lake.main.entity_membership";
pub const ENTITIES: &str = "lake.main.entities";
pub const ENTITY_EVENTS: &str = "lake.main.entity_events";
pub const GOLDEN_LINEAGE: &str = "lake.main.golden_lineage";
pub const GOLDEN_RECORDS: &str = "lake.main.golden_records";
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
    let edges: Vec<(String, String)> = session
        .query_strings(&format!(
            "select rec_a_key, rec_b_key from {MATCH_SCORES} \
             where is_active and match_probability >= {auto_merge} order by rec_a_key, rec_b_key"
        ))?
        .into_iter()
        .map(|r| (r[0].clone(), r[1].clone()))
        .collect();
    let nodes: Vec<String> = session
        .query_strings(&format!(
            "select record_key from {INT_STD_RECORDS} order by record_key"
        ))?
        .into_iter()
        .map(|r| r[0].clone())
        .collect();

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
