//! `content_hash` — the normative S4.1 digest (Rust port of
//! `src/er/ingest/hashing.py`).
//!
//! Each value is NFC-normalized, NULL encodes as the empty string, values are
//! joined by the `0x1f` unit separator and SHA-256'd to lowercase hex. The
//! optional `metadata-v1` extension folds sorted unmapped field names + verbatim
//! values in. Validated bit-for-bit against the committed golden vectors
//! (`tests/content_hash_vectors.json`, vendored from the Python repo).

pub mod std_hash;

use std::collections::BTreeMap;

use sha2::{Digest, Sha256};
use unicode_normalization::UnicodeNormalization;

/// S4.1: source values are joined by the `0x1f` unit separator.
pub const UNIT_SEPARATOR: char = '\u{1f}';
/// S4.1.1: a tombstone version row carries this sentinel; `content_hash` can
/// never produce it (it always hashes at least one separator-joined value).
pub const TOMBSTONE_CONTENT_HASH: &str =
    "0000000000000000000000000000000000000000000000000000000000000000";

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

/// The S4.1 `content_hash` of `row` over `columns` (in declared order), with an
/// optional `metadata-v1` extension over `metadata_columns`.
///
/// A column absent from `row`, or present as NULL, encodes as the empty string
/// — so NULL and "" in the same position hash identically (by design). Keys in
/// `row` that are not in `columns`/`metadata_columns` cannot reach the preimage.
#[must_use]
pub fn content_hash(
    row: &BTreeMap<String, Option<String>>,
    columns: &[String],
    metadata_columns: &[String],
) -> String {
    let values: Vec<String> = columns
        .iter()
        .map(|column| match row.get(column) {
            Some(Some(value)) => value.nfc().collect::<String>(),
            _ => String::new(),
        })
        .collect();
    let separator = UNIT_SEPARATOR.to_string();
    let digest = sha256_hex(values.join(&separator).as_bytes());
    if metadata_columns.is_empty() {
        return digest;
    }
    // Metadata is verbatim: fixed-length name/value hashes + presence tags avoid
    // delimiter ambiguity. Names sorted and deduped.
    let mut names: Vec<&String> = metadata_columns.iter().collect();
    names.sort_unstable();
    names.dedup();
    let mut extras = String::new();
    for name in names {
        extras.push_str(&sha256_hex(name.as_bytes()));
        match row.get(name).and_then(Option::as_deref) {
            None => extras.push('N'),
            Some(value) => {
                extras.push('V');
                extras.push_str(&sha256_hex(value.as_bytes()));
            }
        }
    }
    let preimage = format!("{digest}{UNIT_SEPARATOR}metadata-v1{UNIT_SEPARATOR}{extras}");
    sha256_hex(preimage.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cols(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn ascii_baseline_null_equals_empty_and_tombstone() {
        let mut row = BTreeMap::new();
        row.insert("first_name".to_string(), Some("Ada".to_string()));
        row.insert("last_name".to_string(), Some("Lovelace".to_string()));
        row.insert(
            "email_address".to_string(),
            Some("ada@example.com".to_string()),
        );
        let columns = cols(&["first_name", "last_name", "email_address"]);
        assert_eq!(
            content_hash(&row, &columns, &[]),
            "78438bfa3dc88703607e4ef9a156aeb4d65065f64cf92fe7906db1239bb98011"
        );

        let mut as_null = row.clone();
        as_null.insert("last_name".to_string(), None);
        let mut as_empty = row.clone();
        as_empty.insert("last_name".to_string(), Some(String::new()));
        assert_eq!(
            content_hash(&as_null, &columns, &[]),
            content_hash(&as_empty, &columns, &[])
        );

        assert_eq!(TOMBSTONE_CONTENT_HASH.len(), 64);
    }
}
