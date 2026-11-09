//! Session landing: ingest base_10's base delivery into a `raw_records` session
//! table and confirm the row count + that content_hash lands correctly (spot-
//! checked against the Python golden).

use dedupe_core::session::{Session, SessionOptions};
use er_config::load_config_str;
use er_ingest::landing::{
    create_raw_records_sql, land_base_delivery, land_full_refresh, row_content_hash,
};
use std::collections::BTreeMap;
use std::fmt::Write as _;

#[test]
fn lands_base_10_into_raw_records() {
    let config = load_config_str(include_str!("fixtures/default.yaml")).unwrap();
    let session = Session::open_in_memory(SessionOptions::deterministic()).unwrap();
    session
        .execute_batch("ATTACH ':memory:' AS lake;")
        .and_then(|()| session.execute_batch(&create_raw_records_sql("lake.main.raw_records")))
        .unwrap();

    let sources = [
        ("crm", include_str!("fixtures/base_10/base/crm.csv")),
        ("billing", include_str!("fixtures/base_10/base/billing.csv")),
        (
            "webforms",
            include_str!("fixtures/base_10/base/webforms.csv"),
        ),
    ];
    let mut total = 0;
    for (source, csv_text) in sources {
        let spec = &config.sources[source];
        total += land_base_delivery(
            &session,
            source,
            spec,
            csv_text,
            "01BATCH0000000000000000BASE",
            "2026-01-01 00:00:00",
        )
        .unwrap();
    }
    assert_eq!(total, 23);
    assert_eq!(session.count("lake.main.raw_records").unwrap(), 23);

    // content_hash landed correctly: spot-check crm C001 against the golden.
    let golden: serde_json::Value =
        serde_json::from_str(include_str!("raw_records_golden.json")).unwrap();
    let want = golden["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["source_system"] == "crm" && c["source_record_id"] == "C001")
        .expect("crm C001 in golden")["content_hash"]
        .as_str()
        .unwrap();
    let got = session
        .query_strings(
            "select content_hash from lake.main.raw_records \
             where source_system = 'crm' and source_record_id = 'C001'",
        )
        .unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0][0], want, "landed content_hash for crm:C001 differs");
}

#[test]
fn duplicate_headers_fail_before_landing_and_distinct_headers_preserve_fields() {
    let config = load_config_str(include_str!("fixtures/default.yaml")).unwrap();
    let session = Session::open_in_memory(SessionOptions::deterministic()).unwrap();
    session.execute_batch("ATTACH ':memory:' AS lake;").unwrap();
    session
        .execute_batch(&create_raw_records_sql("lake.main.raw_records"))
        .unwrap();

    for (csv, duplicate) in [
        ("crm_id,first_name,first_name\nC1,Alice,Eve\n", "first_name"),
        ("crm_id,note,note\nC1,one,two\n", "note"),
    ] {
        let error = land_base_delivery(
            &session,
            "crm",
            &config.sources["crm"],
            csv,
            "batch",
            "2026-01-01",
        )
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains(&format!("duplicate CSV header `{duplicate}`"))
        );
        assert_eq!(session.count("lake.main.raw_records").unwrap(), 0);
    }

    let distinct = "crm_id,first_name,note\nC1,Alice,one\n";
    assert_eq!(
        land_base_delivery(
            &session,
            "crm",
            &config.sources["crm"],
            distinct,
            "batch",
            "2026-01-01",
        )
        .unwrap(),
        1
    );
    let rows = session
        .query_strings("SELECT payload FROM lake.main.raw_records")
        .unwrap();
    let payload: serde_json::Value = serde_json::from_str(&rows[0][0]).unwrap();
    assert_eq!(payload["crm_id"], "C1");
    assert_eq!(payload["first_name"], "Alice");
    assert_eq!(payload["note"], "one");
}

#[test]
fn failed_late_append_chunk_rolls_back_prior_chunks_and_retries_cleanly() {
    let config = load_config_str(include_str!("fixtures/default.yaml")).unwrap();
    let session = Session::open_in_memory(SessionOptions::deterministic()).unwrap();
    let ddl = create_raw_records_sql("lake.main.raw_records").replace(
        "updated_at timestamp)",
        "updated_at timestamp, PRIMARY KEY(id))",
    );
    session.execute_batch("ATTACH ':memory:' AS lake;").unwrap();
    session.execute_batch(&ddl).unwrap();

    let mut csv = String::from("crm_id,first_name\n");
    for i in 0..1000 {
        writeln!(csv, "C{i:03},Name{i}").unwrap();
    }
    let valid_csv = csv.clone();
    // The duplicate id lands in the third 500-row INSERT after two flushed
    // buffers, and the transaction must roll those earlier inserts back.
    csv.push_str("C000,Name0\n");
    assert!(
        land_base_delivery(
            &session,
            "crm",
            &config.sources["crm"],
            &csv,
            "failed-batch",
            "2026-01-01",
        )
        .is_err()
    );
    assert_eq!(session.count("lake.main.raw_records").unwrap(), 0);

    assert_eq!(
        land_base_delivery(
            &session,
            "crm",
            &config.sources["crm"],
            &valid_csv,
            "failed-batch",
            "2026-01-01",
        )
        .unwrap(),
        1000
    );
    assert_eq!(session.count("lake.main.raw_records").unwrap(), 1000);
}

#[test]
fn large_delivery_preserves_count_and_content_hash_across_buffer_flushes() {
    let config = load_config_str(include_str!("fixtures/default.yaml")).unwrap();
    let spec = &config.sources["crm"];
    let session = Session::open_in_memory(SessionOptions::deterministic()).unwrap();
    session.execute_batch("ATTACH ':memory:' AS lake;").unwrap();
    session
        .execute_batch(&create_raw_records_sql("lake.main.raw_records"))
        .unwrap();

    let mut csv = String::from("crm_id,first_name,note\n");
    for i in 0..1201 {
        writeln!(csv, "C{i:04},Name{i},note{i}").unwrap();
    }
    assert_eq!(
        land_base_delivery(&session, "crm", spec, &csv, "large", "2026-01-01").unwrap(),
        1201
    );
    assert_eq!(session.count("lake.main.raw_records").unwrap(), 1201);
    for i in [0, 499, 500, 999, 1000, 1200] {
        let id = format!("C{i:04}");
        let payload = BTreeMap::from([
            ("crm_id".to_string(), Some(id.clone())),
            ("first_name".to_string(), Some(format!("Name{i}"))),
            ("note".to_string(), Some(format!("note{i}"))),
        ]);
        let expected = row_content_hash(&payload, spec);
        let rows = session
            .query_strings(&format!(
                "SELECT content_hash FROM lake.main.raw_records WHERE source_record_id = '{id}'"
            ))
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0][0], expected, "content hash changed at row {i}");
    }
}

#[test]
fn failed_full_refresh_tombstone_rolls_back_delivery_and_retries_cleanly() {
    let config = load_config_str(include_str!("fixtures/default.yaml")).unwrap();
    let session = Session::open_in_memory(SessionOptions::deterministic()).unwrap();
    let ddl = create_raw_records_sql("lake.main.raw_records").replace(
        "updated_at timestamp)",
        "updated_at timestamp, CHECK (is_deleted = false))",
    );
    session.execute_batch("ATTACH ':memory:' AS lake;").unwrap();
    session.execute_batch(&ddl).unwrap();

    let base = "crm_id,first_name\nA,Alice\nB,Bob\n";
    land_base_delivery(
        &session,
        "crm",
        &config.sources["crm"],
        base,
        "base",
        "2026-01-01",
    )
    .unwrap();
    let incomplete = "crm_id,first_name\nC,Carol\n";
    assert!(
        land_full_refresh(
            &session,
            "crm",
            &config.sources["crm"],
            incomplete,
            "refresh",
            "2026-01-02",
        )
        .is_err()
    );
    assert_eq!(session.count("lake.main.raw_records").unwrap(), 2);

    let complete = "crm_id,first_name\nA,Alice\nB,Bob\nC,Carol\n";
    assert_eq!(
        land_full_refresh(
            &session,
            "crm",
            &config.sources["crm"],
            complete,
            "refresh",
            "2026-01-02",
        )
        .unwrap(),
        3
    );
    assert_eq!(session.count("lake.main.raw_records").unwrap(), 5);
}

#[test]
fn invalid_record_key_components_never_land() {
    let config = load_config_str(include_str!("fixtures/default.yaml")).unwrap();
    let spec = &config.sources["crm"];
    let session = Session::open_in_memory(SessionOptions::deterministic()).unwrap();
    session.execute_batch("ATTACH ':memory:' AS lake;").unwrap();
    session
        .execute_batch(&create_raw_records_sql("lake.main.raw_records"))
        .unwrap();

    for (source, csv) in [
        ("", "crm_id,first_name\nC1,Alice\n"),
        ("a:b", "crm_id,first_name\nC1,Alice\n"),
        ("crm", "crm_id,first_name\n,Alice\n"),
        ("crm", "crm_id,first_name\nA:B,Alice\n"),
    ] {
        let error =
            land_base_delivery(&session, source, spec, csv, "bad", "2026-01-01").unwrap_err();
        assert!(
            format!("{error:#}").contains("must not"),
            "{source:?}: {error:#}"
        );
        assert_eq!(session.count("lake.main.raw_records").unwrap(), 0);
    }

    assert_eq!(
        land_base_delivery(
            &session,
            "crm",
            spec,
            "crm_id,first_name\nC1,Alice\n",
            "valid",
            "2026-01-01",
        )
        .unwrap(),
        1
    );
    let keys = session
        .query_strings("select source_system, source_record_id from lake.main.raw_records")
        .unwrap();
    assert_eq!(keys, vec![vec!["crm".to_string(), "C1".to_string()]]);
}
