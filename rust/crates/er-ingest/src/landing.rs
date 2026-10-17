//! Landing a delivery into `raw_records` (S4.1) and the `content_hash` preimage.

use std::collections::{BTreeMap, BTreeSet};

use dedupe_core::session::Session;
use er_config::SourceSpec;

/// The `raw_records` columns (S4.1), in S5 DDL order.
pub const RAW_RECORDS_COLUMNS: [&str; 8] = [
    "source_system",
    "source_record_id",
    "payload",
    "content_hash",
    "is_deleted",
    "deleted_at",
    "ingest_batch_id",
    "ingested_at",
];

/// The S4.1.1 tombstone sentinel `content_hash`: 64 zeros. The S4.1 hash function
/// always hashes at least one separator-joined value, so a content version can
/// never collide with it.
pub const TOMBSTONE_CONTENT_HASH: &str =
    "0000000000000000000000000000000000000000000000000000000000000000";

/// The unmapped delivered fields, sorted (S4.2): the delivered keys minus the
/// mapped canonical columns, the record-id column, the updated-at column, and the
/// reserved `persona_id`. This is both `source_metadata`'s retained set and the
/// `metadata-v1` extension of [`row_content_hash`]. Entity-agnostic:
/// `spec.columns.values()` is exactly the mapped canonical source columns for the
/// source's entity type (person nine or Account seven); a person spec yields the
/// identical set as before (this is a sorted membership set, order-independent).
#[must_use]
pub fn metadata_columns(delivered: &[&str], spec: &SourceSpec) -> Vec<String> {
    let mut excluded: BTreeSet<&str> = spec.columns.values().map(String::as_str).collect();
    excluded.insert(spec.record_id_column.as_str());
    excluded.insert(spec.updated_at_column.as_str());
    excluded.insert("persona_id");
    let mut names: Vec<String> = delivered
        .iter()
        .filter(|key| !excluded.contains(**key))
        .map(|key| (*key).to_string())
        .collect();
    names.sort();
    names.dedup();
    names
}

/// The S4.1 `content_hash` of one delivered row: the mapped column VALUES in the
/// source's declared order (`spec.columns.values()`), joined and SHA-256'd, with
/// the unmapped fields as the `metadata-v1` extension. A mapped column absent from
/// the row, or NULL, hashes as the empty string.
#[must_use]
pub fn row_content_hash(payload: &BTreeMap<String, Option<String>>, spec: &SourceSpec) -> String {
    let columns: Vec<String> = spec.columns.values().cloned().collect();
    let delivered: Vec<&str> = payload.keys().map(String::as_str).collect();
    let metadata = metadata_columns(&delivered, spec);
    er_hash::content_hash(payload, &columns, &metadata)
}

/// DDL for a `raw_records` table (S4.1 shape). For the base-delivery gate this is
/// a plain session table, not a DuckLake relation (the stage SQL references it by
/// name, so the lake attachment is not needed to prove standardization parity).
#[must_use]
pub fn create_raw_records_sql(table: &str) -> String {
    format!(
        "create table {table} (\
         source_system varchar, source_record_id varchar, payload json, \
         content_hash varchar, is_deleted boolean, deleted_at timestamp, \
         ingest_batch_id varchar, ingested_at timestamp)"
    )
}

/// Land one source's base-delivery CSV into `lake.main.raw_records` (S4.1): one
/// row per record, `payload` the full delivered row as JSON, `content_hash` per
/// [`row_content_hash`]. The reserved `persona_id` is stripped. Rows cross
/// through the generated bulk-append primitive (typed, bound parameters — the
/// tuple shape comes from the entity, never hand-written). Returns the number
/// of rows landed.
///
/// `ingest_batch_id`/`ingested_at` are stamped verbatim; neither is in
/// `STD_HASH_COLUMNS`, so their values never reach `std_hash`.
pub fn land_base_delivery(
    session: &Session,
    source: &str,
    spec: &SourceSpec,
    csv_text: &str,
    ingest_batch_id: &str,
    ingested_at: &str,
) -> anyhow::Result<usize> {
    let mut reader = csv::Reader::from_reader(csv_text.as_bytes());
    let headers: Vec<String> = reader.headers()?.iter().map(str::to_string).collect();

    let mut rows: Vec<er_entities::raw_record::AppendRow> = Vec::new();
    for record in reader.records() {
        let record = record?;
        let mut payload: BTreeMap<String, Option<String>> = BTreeMap::new();
        for (header, field) in headers.iter().zip(record.iter()) {
            if header == "persona_id" {
                continue;
            }
            payload.insert(header.clone(), Some(field.to_string()));
        }
        let source_record_id = payload
            .get(&spec.record_id_column)
            .and_then(Option::as_deref)
            .ok_or_else(|| {
                anyhow::anyhow!("row missing record-id column {}", spec.record_id_column)
            })?
            .to_string();
        let content_hash = row_content_hash(&payload, spec);
        let json_map: serde_json::Map<String, serde_json::Value> = payload
            .iter()
            .map(|(k, v)| {
                (
                    k.clone(),
                    serde_json::Value::String(v.clone().unwrap_or_default()),
                )
            })
            .collect();
        let payload_json = serde_json::to_string(&serde_json::Value::Object(json_map))?;
        rows.push(er_entities::raw_record::AppendRow {
            source_system: source.to_string(),
            source_record_id,
            payload: Some(payload_json),
            content_hash,
            is_deleted: false,
            deleted_at: None,
            ingest_batch_id: ingest_batch_id.to_string(),
            ingested_at: ingested_at.to_string(),
        });
    }

    er_entities::raw_record::append_rows(session.connection(), &rows)
        .map_err(|e| anyhow::anyhow!("append into raw_records: {e}"))
}

/// Land a full-refresh delivery (S4.1.1, `--full-refresh-keys`): land the
/// delivered rows, then TOMBSTONE every pre-existing key of this source the
/// delivery omits — append a row carrying [`TOMBSTONE_CONTENT_HASH`], stamped at
/// `ingested_at` so it supersedes the live version. The standardization stage
/// (`int_std_records`) then drops those keys, so an omitted record leaves the
/// corpus (S4.5.5: `member_removed`, and `retired` when it empties an entity).
/// Returns the number of delivered rows landed (tombstones excluded).
///
/// # Errors
/// Propagates the CSV parse / session errors of [`land_base_delivery`] and the
/// omitted-key query / tombstone insert.
pub fn land_full_refresh(
    session: &Session,
    source: &str,
    spec: &SourceSpec,
    csv_text: &str,
    ingest_batch_id: &str,
    ingested_at: &str,
) -> anyhow::Result<usize> {
    let landed = land_base_delivery(
        session,
        source,
        spec,
        csv_text,
        ingest_batch_id,
        ingested_at,
    )?;
    // Keys this source carried before this delivery but that the delivery omits
    // (the delivered rows are already landed under this batch id, so the omitted
    // set is a pure self-subquery — the declared live statement).
    let omitted = er_entities::raw_record::read::omitted_keys_live(
        session.connection(),
        source,
        ingest_batch_id,
    )
    .map_err(|e| anyhow::anyhow!("full-refresh omitted-key query: {e}"))?;
    if !omitted.is_empty() {
        let rows: Vec<er_entities::raw_record::AppendRow> = omitted
            .into_iter()
            .map(|r| er_entities::raw_record::AppendRow {
                source_system: source.to_string(),
                source_record_id: r.source_record_id,
                payload: None,
                content_hash: TOMBSTONE_CONTENT_HASH.to_string(),
                is_deleted: true,
                deleted_at: Some(ingested_at.to_string()),
                ingest_batch_id: ingest_batch_id.to_string(),
                ingested_at: ingested_at.to_string(),
            })
            .collect();
        er_entities::raw_record::append_rows(session.connection(), &rows)
            .map_err(|e| anyhow::anyhow!("append tombstones into raw_records: {e}"))?;
    }
    Ok(landed)
}

/// A per-source delivery receipt (S4.1): how this batch's landed keys compare to
/// the source's prior state. `raw_records` is append-only, so the breakdown is a
/// POST-landing comparison, not a byproduct of the insert.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BatchReceipt {
    pub new: i64,
    pub changed: i64,
    pub unchanged: i64,
    pub tombstone: i64,
    pub resurrected: i64,
}

/// Classify this batch's landed rows for `source` in `table` against the source's
/// prior landed state — the rows of this source NOT carrying `ingest_batch_id`,
/// reduced to the latest row per key (`ingested_at`, then `ingest_batch_id`).
///
/// A delivered (non-tombstone) key is `new` if its key was unseen, `resurrected`
/// if the prior latest was a tombstone, `unchanged` if the content hash is
/// identical, else `changed`. A tombstone row (appended by [`land_full_refresh`]
/// for an omitted key) counts as `tombstone` only when it retires a key whose
/// prior latest was live — re-tombstoning an already-dead key is not a removal.
/// Every delivered key falls in exactly one of new/resurrected/unchanged/changed.
///
/// # Errors
/// Propagates the aggregate-query error.
pub fn batch_receipt(
    session: &Session,
    source: &str,
    ingest_batch_id: &str,
) -> anyhow::Result<BatchReceipt> {
    let rows = er_entities::raw_record::read::batch_receipt_live(
        session.connection(),
        source,
        ingest_batch_id,
        TOMBSTONE_CONTENT_HASH,
    )
    .map_err(|e| anyhow::anyhow!("batch receipt: {e}"))?;
    let r = rows
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("empty batch-receipt result"))?;
    Ok(BatchReceipt {
        new: r.new_count.unwrap_or(0),
        resurrected: r.resurrected_count.unwrap_or(0),
        unchanged: r.unchanged_count.unwrap_or(0),
        changed: r.changed_count.unwrap_or(0),
        tombstone: r.tombstone_count.unwrap_or(0),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tombstone_is_64_zeros_and_columns_are_eight() {
        assert_eq!(TOMBSTONE_CONTENT_HASH.len(), 64);
        assert!(TOMBSTONE_CONTENT_HASH.chars().all(|c| c == '0'));
        assert_eq!(RAW_RECORDS_COLUMNS.len(), 8);
        assert_eq!(RAW_RECORDS_COLUMNS[0], "source_system");
    }

    /// The session-table DDL and column list here must stay in S5 order: the
    /// oracle against the registry spec that owns `lake.main.raw_records` (the
    /// lake relation itself is created from that spec by er-cli).
    #[test]
    fn session_ddl_matches_the_s5_registry() {
        let spec = er_schema::model::all_specs()
            .into_iter()
            .find(|s| s.name == "raw_records")
            .expect("raw_records is in the registry");
        assert_eq!(spec.column_names(), RAW_RECORDS_COLUMNS);
        let ddl = create_raw_records_sql("t");
        for column in &spec.columns {
            assert!(
                ddl.contains(&format!("{} {}", column.name, column.r#type.to_lowercase())),
                "session DDL is missing {} {}",
                column.name,
                column.r#type
            );
        }
    }
}
