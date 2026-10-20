//! One-off: materialize base_10's standardized corpus to a committed Parquet
//! fixture, the shared scoring input for the Python match-score oracle
//! (`gen_match_golden.py`) and this crate's matching gate. Parquet (not CSV)
//! because the corpus carries a `name_variants LIST(VARCHAR)` and a `birth_date
//! DATE` the model's comparisons read natively (`list_intersect`,
//! `date_trunc`), which CSV would flatten to text.
//!
//! The corpus is Rust-standardized; `er-standardize`'s std_hashes gate proves
//! that output byte-exact vs Python over this same delivery, so Python scoring
//! this Parquet and Rust scoring its own in-session `int_std_records` start from
//! the identical corpus. Regenerate with:
//!   cargo test -p er-match --test dump_corpus -- --ignored

mod common;

use dedupe_core::session::{Session, SessionOptions};

/// MATCHING_RECORD_COLUMNS (src/er/lake/columns.py): the corpus the matcher scores,
/// in declared order.
const MATCHING_RECORD_COLUMNS: &str = "record_key, source_system, source_record_id, \
     content_hash, std_version, given_name, family_name, name_variants, email, email_valid, \
     phone_e164, phone_valid, addr_number, addr_street, addr_unit, addr_city, addr_region, \
     addr_postal, birth_date, updated_at_source, ingest_batch_id, ingested_at";

#[test]
#[ignore = "regenerates a committed fixture; run explicitly with --ignored"]
fn dump_base_10_corpus() {
    let session = Session::open_in_memory(SessionOptions::deterministic()).unwrap();
    common::land_and_standardize_base_10(&session);

    let out = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/int_std_records_base_10.parquet"
    );
    let path = out.replace('\'', "''");
    session
        .execute_batch(&format!(
            "copy (select {MATCHING_RECORD_COLUMNS} from int_std_records order by record_key) \
             to '{path}' (format parquet)"
        ))
        .unwrap();
    assert!(
        std::path::Path::new(out).exists(),
        "parquet corpus not written to {out}"
    );
}
