//! Bit-for-bit parity against the committed S4.1 golden vectors (vendored from
//! the Python repo's `tests/fixtures/content_hash_vectors.json`). A change that
//! alters any digest fails here rather than silently rewriting the contract.

use std::collections::BTreeMap;

use er_hash::{TOMBSTONE_CONTENT_HASH, UNIT_SEPARATOR, content_hash};

#[test]
fn golden_vectors_reproduce_content_hash() {
    let raw = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/content_hash_vectors.json"
    ));
    let doc: serde_json::Value = serde_json::from_str(raw).expect("valid vectors json");

    assert_eq!(
        doc["tombstone_content_hash"].as_str().unwrap(),
        TOMBSTONE_CONTENT_HASH
    );
    assert_eq!(
        doc["separator_hex"].as_str().unwrap(),
        format!("{:02x}", UNIT_SEPARATOR as u32)
    );

    let vectors = doc["vectors"].as_array().expect("vectors array");
    let count = vectors.len();
    assert!(count > 0, "golden vectors file is empty");
    for vector in vectors {
        let name = vector["name"].as_str().unwrap_or("?");
        let columns: Vec<String> = vector["columns"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c.as_str().unwrap().to_string())
            .collect();
        let mut row: BTreeMap<String, Option<String>> = BTreeMap::new();
        for (key, value) in vector["row"].as_object().unwrap() {
            row.insert(
                key.clone(),
                if value.is_null() {
                    None
                } else {
                    Some(value.as_str().unwrap().to_string())
                },
            );
        }
        let metadata_columns: Vec<String> = vector
            .get("metadata_columns")
            .and_then(serde_json::Value::as_array)
            .map(|a| a.iter().map(|c| c.as_str().unwrap().to_string()).collect())
            .unwrap_or_default();

        let got = content_hash(&row, &columns, &metadata_columns);
        assert_eq!(got, vector["digest"].as_str().unwrap(), "vector {name}");
    }
}
