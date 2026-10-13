//! `std_hash` — the T-STD-1 content hash over an `int_std_records` row (port of
//! `src/er/lake/hashing.py`). Same digest as `content_hash`, different column
//! list, plus a type-rendering step: BOOLEAN→`true`/`false`, TIMESTAMP/DATE→ISO
//! text (modelled as `Text`), LIST→`0x1f`-join, NULL→`""`.

use std::collections::BTreeMap;

use crate::{UNIT_SEPARATOR, content_hash};

/// The T-STD-1 projection of S8.3, in its literal order (the preimage order —
/// not sorted, not derived; reordering invalidates every committed std_hash).
pub const STD_HASH_COLUMNS: [&str; 18] = [
    "record_key",
    "std_version",
    "given_name",
    "family_name",
    "name_variants",
    "email",
    "email_valid",
    "phone_e164",
    "phone_valid",
    "addr_number",
    "addr_street",
    "addr_unit",
    "addr_city",
    "addr_region",
    "addr_postal",
    "birth_date",
    "updated_at_source",
    "content_hash",
];

/// An `int_std_records` value, reduced to the text T-STD-1 hashes. DATE and
/// TIMESTAMP are carried as their isoformat `Text` (the rendering the driver
/// boundary performs), so `render` is pure string handling here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StdValue {
    Null,
    Text(String),
    Bool(bool),
    List(Vec<String>),
}

impl StdValue {
    fn render(&self) -> String {
        match self {
            StdValue::Null => String::new(),
            StdValue::Text(s) => s.clone(),
            StdValue::Bool(true) => "true".to_string(),
            StdValue::Bool(false) => "false".to_string(),
            StdValue::List(items) => items.join(&UNIT_SEPARATOR.to_string()),
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum StdHashError {
    #[error(
        "row is missing {missing:?}; the T-STD-1 preimage is the full {total}-column projection of S8.3"
    )]
    MissingColumns { missing: Vec<String>, total: usize },
}

/// The T-STD-1 `std_hash` of one `int_std_records` row. Every member of
/// [`STD_HASH_COLUMNS`] must be present (a missing member is a projection bug,
/// refused rather than treated as NULL); extra keys (e.g. volatile stamps) are
/// ignored, so two rows differing only in a volatile value hash identically.
pub fn table_content_hash(row: &BTreeMap<String, StdValue>) -> Result<String, StdHashError> {
    let missing: Vec<String> = STD_HASH_COLUMNS
        .iter()
        .copied()
        .filter(|column| !row.contains_key(*column))
        .map(str::to_string)
        .collect();
    if !missing.is_empty() {
        return Err(StdHashError::MissingColumns {
            missing,
            total: STD_HASH_COLUMNS.len(),
        });
    }
    let rendered: BTreeMap<String, Option<String>> = STD_HASH_COLUMNS
        .iter()
        .map(|&column| (column.to_string(), Some(row[column].render())))
        .collect();
    let columns: Vec<String> = STD_HASH_COLUMNS.iter().map(|&c| c.to_string()).collect();
    Ok(content_hash(&rendered, &columns, &[]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_column_is_refused() {
        let mut row = BTreeMap::new();
        row.insert("record_key".to_string(), StdValue::Text("x".to_string()));
        assert!(matches!(
            table_content_hash(&row),
            Err(StdHashError::MissingColumns { .. })
        ));
    }

    #[test]
    fn null_and_empty_render_identically() {
        assert_eq!(
            StdValue::Null.render(),
            StdValue::Text(String::new()).render()
        );
        assert_eq!(StdValue::Bool(true).render(), "true");
        assert_eq!(
            StdValue::List(vec!["a".into(), "b".into()]).render(),
            "a\u{1f}b"
        );
    }
}
