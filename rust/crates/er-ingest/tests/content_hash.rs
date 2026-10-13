//! Byte-exact parity of the S4.1 ingest `content_hash` against a golden from the
//! real Python `er.ingest.hashing.content_hash` over base_10's base delivery
//! (vendored config; regenerate with
//! `uv run python rust/crates/er-ingest/tests/gen_raw_records_golden.py`).

use std::collections::BTreeMap;

use er_config::load_config_str;
use er_ingest::landing::row_content_hash;

#[test]
fn base_delivery_content_hash_matches_python() {
    let config = load_config_str(include_str!("fixtures/default.yaml")).unwrap();
    let golden: serde_json::Value =
        serde_json::from_str(include_str!("raw_records_golden.json")).unwrap();
    let cases = golden["cases"].as_array().unwrap();
    let count = cases.len();
    assert!(count > 20, "expected base_10's full delivery");

    for case in cases {
        let source = case["source_system"].as_str().unwrap();
        let rid = case["source_record_id"].as_str().unwrap();
        let spec = &config.sources[source];
        let payload: BTreeMap<String, Option<String>> = case["payload"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v.as_str().map(str::to_string)))
            .collect();
        assert_eq!(
            row_content_hash(&payload, spec),
            case["content_hash"].as_str().unwrap(),
            "content_hash for {source}:{rid} differs from Python",
        );
    }
}
