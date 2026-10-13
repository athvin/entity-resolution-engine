//! Landing a delivery into `raw_records` (S4.1) and the `content_hash` preimage.

use std::collections::{BTreeMap, BTreeSet};

use dedupe_core::session::Session;
use er_config::{CANONICAL_ATTRIBUTES, SourceSpec};

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

/// The unmapped delivered fields, sorted (S4.2): the delivered keys minus the nine
/// mapped canonical columns, the record-id column, the updated-at column, and the
/// reserved `persona_id`. This is both `source_metadata`'s retained set and the
/// `metadata-v1` extension of [`row_content_hash`].
#[must_use]
pub fn metadata_columns(delivered: &[&str], spec: &SourceSpec) -> Vec<String> {
    let mut excluded: BTreeSet<&str> = CANONICAL_ATTRIBUTES
        .iter()
        .map(|attr| spec.columns[*attr].as_str())
        .collect();
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

fn sql_str(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

/// Land one source's base-delivery CSV into `table` (S4.1): one `raw_records` row
/// per record, `payload` the full delivered row as JSON, `content_hash` per
/// [`row_content_hash`]. The reserved `persona_id` is stripped. For a base
/// delivery (empty table) this is a plain insert — the incremental
/// anti-join/tombstone machinery is inert. Returns the number of rows landed.
///
/// `ingest_batch_id`/`ingested_at` are stamped verbatim; neither is in
/// `STD_HASH_COLUMNS`, so their values never reach `std_hash`.
pub fn land_base_delivery(
    session: &Session,
    table: &str,
    source: &str,
    spec: &SourceSpec,
    csv_text: &str,
    ingest_batch_id: &str,
    ingested_at: &str,
) -> anyhow::Result<usize> {
    let mut reader = csv::Reader::from_reader(csv_text.as_bytes());
    let headers: Vec<String> = reader.headers()?.iter().map(str::to_string).collect();

    let mut rows: Vec<String> = Vec::new();
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
        rows.push(format!(
            "({}, {}, {}::json, {}, false, NULL, {}, {}::timestamp)",
            sql_str(source),
            sql_str(&source_record_id),
            sql_str(&payload_json),
            sql_str(&content_hash),
            sql_str(ingest_batch_id),
            sql_str(ingested_at),
        ));
    }

    let count = rows.len();
    if count > 0 {
        session
            .execute_batch(&format!("insert into {table} values {}", rows.join(", ")))
            .map_err(|e| anyhow::anyhow!("insert into {table}: {e}"))?;
    }
    Ok(count)
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
}
