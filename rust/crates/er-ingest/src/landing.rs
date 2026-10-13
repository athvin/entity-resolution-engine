//! Landing a delivery into `raw_records` (S4.1) and the `content_hash` preimage.

use std::collections::{BTreeMap, BTreeSet};

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
