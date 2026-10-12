//! Bit-for-bit parity of `std_hash::table_content_hash` against golden vectors
//! generated from the real Python `er.lake.hashing.table_content_hash`.

use std::collections::BTreeMap;

use er_hash::std_hash::{STD_HASH_COLUMNS, StdValue, table_content_hash};

fn to_stdvalue(j: &serde_json::Value) -> StdValue {
    match j["t"].as_str().unwrap() {
        "null" => StdValue::Null,
        "bool" => StdValue::Bool(j["v"].as_bool().unwrap()),
        "list" => StdValue::List(
            j["v"]
                .as_array()
                .unwrap()
                .iter()
                .map(|e| e.as_str().unwrap().to_string())
                .collect(),
        ),
        "text" => StdValue::Text(j["v"].as_str().unwrap().to_string()),
        other => panic!("unknown StdValue tag {other}"),
    }
}

#[test]
fn golden_vectors_reproduce_std_hash() {
    let raw = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/std_hash_vectors.json"
    ));
    let doc: serde_json::Value = serde_json::from_str(raw).unwrap();
    let vectors = doc["vectors"].as_array().unwrap();
    let count = vectors.len();
    assert!(count > 0, "golden std-hash vectors file is empty");
    for vector in vectors {
        let name = vector["name"].as_str().unwrap_or("?");
        let mut row = BTreeMap::new();
        for (key, value) in vector["row"].as_object().unwrap() {
            row.insert(key.clone(), to_stdvalue(value));
        }
        assert_eq!(
            table_content_hash(&row).unwrap(),
            vector["digest"].as_str().unwrap(),
            "vector {name}"
        );
    }
}

// S5.0 / M7: the projection is a non-volatile subset of int_std_records.
#[test]
fn std_hash_columns_are_nonvolatile_subset() {
    use er_schema::columns::{STD_RECORD_COLUMNS, VOLATILE_COLUMNS};
    for column in STD_HASH_COLUMNS {
        assert!(!VOLATILE_COLUMNS.contains(&column), "{column} is volatile");
        assert!(
            STD_RECORD_COLUMNS.contains(&column),
            "{column} not in STD_RECORD_COLUMNS"
        );
    }
}
