//! Account standardization (1.1b): build `acct_int_std_records` from a tiny
//! Account delivery and assert the Account normalizers fire (company-name suffix
//! stripping, domain scheme/www/path stripping, non-null variants, E.164 phone).
//! Additive — the person path is exercised elsewhere and untouched.

use dedupe_core::session::{Session, SessionOptions};
use er_config::load_config_str;
use er_standardize::build_acct_int_std_records;

const ACCT_CONFIG: &str = include_str!("fixtures/acct_test.yaml");

#[test]
fn acct_standardization_normalizes_company_domain_phone() {
    let config = load_config_str(ACCT_CONFIG).expect("account config parses");
    assert_eq!(config.entity_type, er_config::EntityType::Account);

    let session = Session::open_in_memory(SessionOptions::deterministic()).unwrap();
    session
        .execute_batch(
            "create table raw as select * from (values \
             ('crm','c1', '{\"company\":\"Acme Inc.\",\"website\":\"https://www.acme.com/about\",\"phone\":\"415-555-0100\",\"street\":\"1 Main St\",\"city\":\"SF\",\"state\":\"CA\",\"zip\":\"94105\",\"crm_id\":\"c1\",\"last_modified\":\"2026-01-01\",\"extra\":\"keep\"}'::json, 'h1', 'b1', timestamp '2026-01-01 00:00:00'), \
             ('crm','c2', '{\"company\":\"Beta, LLC\",\"website\":\"beta.io\",\"phone\":\"\",\"street\":\"2 Oak Ave\",\"city\":\"NYC\",\"state\":\"NY\",\"zip\":\"10001\",\"crm_id\":\"c2\",\"last_modified\":\"2026-01-02\",\"extra\":\"x\"}'::json, 'h2', 'b1', timestamp '2026-01-02 00:00:00') \
             ) as t(source_system, source_record_id, payload, content_hash, ingest_batch_id, ingested_at)",
        )
        .unwrap();

    build_acct_int_std_records(&session, &config, "raw", "acct_int_std").unwrap();

    let rows = session
        .query_strings(
            "select record_key, company_name_norm, domain_norm, \
             len(company_variants), phone_e164 \
             from acct_int_std order by record_key",
        )
        .unwrap();

    assert_eq!(rows.len(), 2, "two account records standardized");
    // c1: legal suffix stripped, domain scheme/www/path stripped, variants non-empty, phone E.164.
    assert_eq!(rows[0][0], "crm:c1");
    assert_eq!(rows[0][1], "acme", "company_name_norm strips 'Inc.'");
    assert_eq!(rows[0][2], "acme.com", "domain_norm strips scheme/www/path");
    assert_eq!(rows[0][3], "1", "company_variants is a non-empty list");
    assert_eq!(rows[0][4], "+14155550100", "phone_e164 (US NANP)");
    // c2: comma + LLC stripped; bare domain unchanged.
    assert_eq!(rows[1][0], "crm:c2");
    assert_eq!(rows[1][1], "beta", "company_name_norm strips ', LLC'");
    assert_eq!(rows[1][2], "beta.io");
}
