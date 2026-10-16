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
    table: &str,
    source: &str,
    spec: &SourceSpec,
    csv_text: &str,
    ingest_batch_id: &str,
    ingested_at: &str,
) -> anyhow::Result<usize> {
    let landed = land_base_delivery(
        session,
        table,
        source,
        spec,
        csv_text,
        ingest_batch_id,
        ingested_at,
    )?;
    // Keys this source carried before this delivery but that the delivery omits.
    let omitted = session
        .query_strings(&format!(
            "select distinct source_record_id from {table} \
             where source_system = {src} and ingest_batch_id <> {batch} \
               and source_record_id not in (\
                 select source_record_id from {table} \
                 where source_system = {src} and ingest_batch_id = {batch})",
            src = sql_str(source),
            batch = sql_str(ingest_batch_id),
        ))
        .map_err(|e| anyhow::anyhow!("full-refresh omitted-key query on {table}: {e}"))?;
    if !omitted.is_empty() {
        let rows: Vec<String> = omitted
            .iter()
            .map(|r| {
                format!(
                    "({}, {}, NULL, {}, true, {}::timestamp, {}, {}::timestamp)",
                    sql_str(source),
                    sql_str(&r[0]),
                    sql_str(TOMBSTONE_CONTENT_HASH),
                    sql_str(ingested_at),
                    sql_str(ingest_batch_id),
                    sql_str(ingested_at),
                )
            })
            .collect();
        session
            .execute_batch(&format!("insert into {table} values {}", rows.join(", ")))
            .map_err(|e| anyhow::anyhow!("insert tombstones into {table}: {e}"))?;
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
    table: &str,
    source: &str,
    ingest_batch_id: &str,
) -> anyhow::Result<BatchReceipt> {
    let tomb = TOMBSTONE_CONTENT_HASH;
    let sql = format!(
        "with prior as (\
           select source_record_id, content_hash, \
             row_number() over (partition by source_record_id \
               order by ingested_at desc, ingest_batch_id desc) as rn \
           from {table} where source_system = {src} and ingest_batch_id <> {batch}), \
         prior_latest as (select source_record_id, content_hash from prior where rn = 1), \
         this_batch as (select source_record_id, content_hash from {table} \
           where source_system = {src} and ingest_batch_id = {batch}) \
         select \
           count(*) filter (where tb.content_hash <> '{tomb}' and p.source_record_id is null), \
           count(*) filter (where tb.content_hash <> '{tomb}' and p.content_hash = '{tomb}'), \
           count(*) filter (where tb.content_hash <> '{tomb}' and p.content_hash = tb.content_hash), \
           count(*) filter (where tb.content_hash <> '{tomb}' and p.source_record_id is not null \
             and p.content_hash <> '{tomb}' and p.content_hash <> tb.content_hash), \
           count(*) filter (where tb.content_hash = '{tomb}' and p.source_record_id is not null \
             and p.content_hash <> '{tomb}') \
         from this_batch tb left join prior_latest p on tb.source_record_id = p.source_record_id",
        src = sql_str(source),
        batch = sql_str(ingest_batch_id),
    );
    let rows = session
        .query_strings(&sql)
        .map_err(|e| anyhow::anyhow!("batch receipt on {table}: {e}"))?;
    let r = rows
        .first()
        .ok_or_else(|| anyhow::anyhow!("empty batch-receipt result"))?;
    let n = |i: usize| r.get(i).and_then(|v| v.parse::<i64>().ok()).unwrap_or(0);
    Ok(BatchReceipt {
        new: n(0),
        resurrected: n(1),
        unchanged: n(2),
        changed: n(3),
        tombstone: n(4),
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
}
