//! Session landing: ingest base_10's base delivery into a `raw_records` session
//! table and confirm the row count + that content_hash lands correctly (spot-
//! checked against the Python golden).

use dedupe_core::session::{Session, SessionOptions};
use er_config::load_config_str;
use er_ingest::landing::{create_raw_records_sql, land_base_delivery};

#[test]
fn lands_base_10_into_raw_records() {
    let config = load_config_str(include_str!("fixtures/default.yaml")).unwrap();
    let session = Session::open_in_memory(SessionOptions::deterministic()).unwrap();
    session
        .execute_batch(&create_raw_records_sql("raw_records"))
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
            "raw_records",
            source,
            spec,
            csv_text,
            "01BATCH0000000000000000BASE",
            "2026-01-01 00:00:00",
        )
        .unwrap();
    }
    assert_eq!(total, 23);
    assert_eq!(session.count("raw_records").unwrap(), 23);

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
            "select content_hash from raw_records \
             where source_system = 'crm' and source_record_id = 'C001'",
        )
        .unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0][0], want, "landed content_hash for crm:C001 differs");
}
