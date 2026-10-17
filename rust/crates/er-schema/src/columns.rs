//! The three column sets S5.0 requires to have exactly one definition (port of
//! `src/er/lake/columns.py`). Both tuples are in S5 DDL order — load-bearing,
//! read positionally downstream; neither may be sorted or set-ified. The derived
//! sets are computed from the base arrays so they cannot drift.

use std::sync::LazyLock;

/// The nine run-scoped stamps excluded from every determinism comparison (S5.0).
pub const VOLATILE_COLUMNS: [&str; 9] = [
    "ingest_batch_id",
    "ingested_at",
    "assembled_at",
    "scored_at",
    "assigned_at",
    "occurred_at",
    "run_id",
    "event_id",
    "seq",
];

/// The eleven `golden_records` columns produced by survivorship, in S5 DDL order.
pub const GOLDEN_SURVIVABLE_COLUMNS: [&str; 11] = [
    "given_name",
    "family_name",
    "email",
    "phone_e164",
    "addr_number",
    "addr_street",
    "addr_unit",
    "addr_city",
    "addr_region",
    "addr_postal",
    "birth_date",
];

/// The attribute the six `addr_*` columns are decided under (one survivorship
/// decision, S4.6). Not itself a `golden_records` column.
pub const ADDRESS_ATTRIBUTE: &str = "address";

/// The six `addr_*` columns, derived from [`GOLDEN_SURVIVABLE_COLUMNS`].
pub static ADDRESS_COMPOSITE_COLUMNS: LazyLock<Vec<&'static str>> = LazyLock::new(|| {
    GOLDEN_SURVIVABLE_COLUMNS
        .iter()
        .copied()
        .filter(|c| c.starts_with("addr_"))
        .collect()
});

/// `golden_lineage.attribute`'s closed vocabulary: the survivable set with the
/// six `addr_*` columns collapsed into the single `address` decision, in order.
pub static GOLDEN_LINEAGE_ATTRIBUTES: LazyLock<Vec<&'static str>> = LazyLock::new(|| {
    let mut out: Vec<&'static str> = Vec::new();
    for &column in &GOLDEN_SURVIVABLE_COLUMNS {
        let token = if column.starts_with("addr_") {
            ADDRESS_ATTRIBUTE
        } else {
            column
        };
        if !out.contains(&token) {
            out.push(token);
        }
    }
    out
});

/// The golden marts, in the order S4.6 lists them (rebuilt together; the
/// presentation-only `golden_display` was retired unbuilt).
pub const GOLDEN_MART_RELATIONS: [&str; 2] = ["golden_records", "golden_lineage"];

/// The `int_std_records` column list of S5, in DDL order (`record_key` leads: the
/// logical key and Splink's `unique_id_column_name`).
pub const STD_RECORD_COLUMNS: [&str; 23] = [
    "record_key",
    "source_system",
    "source_record_id",
    "content_hash",
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
    "metadata",
    "updated_at_source",
    "ingest_batch_id",
    "ingested_at",
];

/// Matching inputs: `STD_RECORD_COLUMNS` minus `metadata` (client-only data never
/// enters Splink, blocking or term frequencies).
pub static MATCHING_RECORD_COLUMNS: LazyLock<Vec<&'static str>> = LazyLock::new(|| {
    STD_RECORD_COLUMNS
        .iter()
        .copied()
        .filter(|&c| c != "metadata")
        .collect()
});

/// Whether `column` is a run-scoped stamp excluded from determinism comparisons.
#[must_use]
pub fn is_volatile(column: &str) -> bool {
    VOLATILE_COLUMNS.contains(&column)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_and_s5_order() {
        assert_eq!(VOLATILE_COLUMNS.len(), 9);
        assert_eq!(GOLDEN_SURVIVABLE_COLUMNS.len(), 11);
        assert_eq!(GOLDEN_SURVIVABLE_COLUMNS[0], "given_name");
        assert_eq!(GOLDEN_SURVIVABLE_COLUMNS[10], "birth_date");
        assert_eq!(STD_RECORD_COLUMNS.len(), 23);
        assert_eq!(STD_RECORD_COLUMNS[0], "record_key");
        assert_eq!(STD_RECORD_COLUMNS[19], "metadata");
        assert_eq!(GOLDEN_MART_RELATIONS, ["golden_records", "golden_lineage"]);
    }

    #[test]
    fn derivations_by_construction() {
        assert_eq!(
            *ADDRESS_COMPOSITE_COLUMNS,
            vec![
                "addr_number",
                "addr_street",
                "addr_unit",
                "addr_city",
                "addr_region",
                "addr_postal"
            ]
        );
        assert_eq!(
            *GOLDEN_LINEAGE_ATTRIBUTES,
            vec![
                "given_name",
                "family_name",
                "email",
                "phone_e164",
                "address",
                "birth_date"
            ]
        );
        assert_eq!(MATCHING_RECORD_COLUMNS.len(), 22);
        assert!(!MATCHING_RECORD_COLUMNS.contains(&"metadata"));
        assert!(STD_RECORD_COLUMNS.contains(&"metadata"));
    }

    #[test]
    fn volatile_membership() {
        assert!(is_volatile("run_id"));
        assert!(is_volatile("seq"));
        assert!(!is_volatile("record_key"));
    }
}
