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

use anyhow::Result;
use dedupe_core::Session;
use dedupe_core::session::SessionOptions;
use dedupe_core::session::ducklake::DuckLakeConfig;
use er_config::Config;
use er_ingest::landing::{create_raw_records_sql, land_base_delivery, land_full_refresh};
use er_standardize::build_int_std_records;

/// The lake relations this crate reads/writes, fully qualified.
pub const RAW_RECORDS: &str = "lake.main.raw_records";
pub const INT_STD_RECORDS: &str = "lake.main.int_std_records";
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
