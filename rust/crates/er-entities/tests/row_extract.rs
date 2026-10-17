//! Round-trip proof for the lake-read batch support module and the first
//! generated `returns: columns` row struct, against a real DuckDB: all eight
//! vocabulary types extract with their declared Rust types, SQL NULL maps to
//! `None`, violations are `Err` (never a panic), and serde renders the row the
//! way the HTTP layer needs (explicit nulls, nested JSON).

use arrow::array::{
    BooleanArray, Date32Array, Float64Array, Int64Array, ListArray, StringArray,
    TimestampMicrosecondArray,
};
use arrow::record_batch::RecordBatch;
use er_entities::batch;
use serde_json::json;

fn query_batches(conn: &duckdb::Connection, sql: &str) -> Vec<RecordBatch> {
    conn.prepare(sql)
        .expect("prepare")
        .query_arrow([])
        .expect("query_arrow")
        .collect()
}

#[test]
fn typed_extraction_round_trips_all_eight_types() {
    let conn = duckdb::Connection::open_in_memory().expect("open duckdb");
    let batches = query_batches(
        &conn,
        "SELECT * FROM (VALUES \
           ('a', 1::BIGINT, 1.5::DOUBLE, true, DATE '2024-01-02', \
            TIMESTAMP '2024-01-02 03:04:05.123456', '{\"k\":1}', ['x','y']), \
           (NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL) \
         ) t(s, n, d, b, dt, ts, j, l) ORDER BY s NULLS LAST",
    );
    assert_eq!(batches.len(), 1, "two rows fit one batch");
    let batch = &batches[0];

    let s: &StringArray = batch::col(batch, 0, "s").expect("s");
    assert_eq!(batch::opt_str(s, 0), Some("a".to_string()));
    assert_eq!(batch::opt_str(s, 1), None);

    let n: &Int64Array = batch::col(batch, 1, "n").expect("n");
    assert_eq!(batch::opt_i64(n, 0), Some(1));
    assert_eq!(batch::opt_i64(n, 1), None);

    let d: &Float64Array = batch::col(batch, 2, "d").expect("d");
    assert_eq!(batch::opt_f64(d, 0), Some(1.5));
    assert_eq!(batch::opt_f64(d, 1), None);

    let b: &BooleanArray = batch::col(batch, 3, "b").expect("b");
    assert_eq!(batch::opt_bool(b, 0), Some(true));
    assert_eq!(batch::opt_bool(b, 1), None);

    let dt: &Date32Array = batch::col(batch, 4, "dt").expect("dt");
    assert_eq!(
        batch::opt_date(dt, 0),
        Some(chrono::NaiveDate::from_ymd_opt(2024, 1, 2).unwrap())
    );
    assert_eq!(batch::opt_date(dt, 1), None);

    let ts: &TimestampMicrosecondArray = batch::col(batch, 5, "ts").expect("ts");
    let got = batch::opt_ts(ts, 0).expect("timestamp value");
    assert_eq!(
        got,
        chrono::DateTime::<chrono::Utc>::from_naive_utc_and_offset(
            chrono::NaiveDate::from_ymd_opt(2024, 1, 2)
                .unwrap()
                .and_hms_micro_opt(3, 4, 5, 123_456)
                .unwrap(),
            chrono::Utc,
        )
    );
    assert_eq!(batch::opt_ts(ts, 1), None);

    let j: &StringArray = batch::col(batch, 6, "j").expect("j");
    assert_eq!(
        batch::opt_json(j, 0, "j").expect("json parses"),
        Some(json!({"k": 1}))
    );
    assert_eq!(batch::opt_json(j, 1, "j").expect("null json"), None);

    let l: &ListArray = batch::col(batch, 7, "l").expect("l");
    assert_eq!(
        batch::opt_list_str(l, 0, "l").expect("list extracts"),
        Some(vec!["x".to_string(), "y".to_string()])
    );
    assert_eq!(batch::opt_list_str(l, 1, "l").expect("null list"), None);

    // A declared non-null column meeting SQL NULL is an Err, not a panic.
    let err = batch::req(batch::opt_str(s, 1), "s", 1).expect_err("NULL in non-null");
    assert!(err.to_string().contains("NULL in a non-null column"));

    // The downcast rejects a wrong array type with an Err.
    assert!(batch::col::<Int64Array>(batch, 0, "s").is_err());
}

#[test]
fn check_columns_rejects_a_shape_drift() {
    let conn = duckdb::Connection::open_in_memory().expect("open duckdb");
    let batches = query_batches(&conn, "SELECT 'a' AS s, 1::BIGINT AS n");
    let batch = &batches[0];

    use arrow::datatypes::DataType;
    batch::check_columns(batch, &[("s", DataType::Utf8), ("n", DataType::Int64)])
        .expect("the matching shape passes");
    assert!(
        batch::check_columns(batch, &[("s", DataType::Utf8)]).is_err(),
        "a missing column is a shape error"
    );
    assert!(
        batch::check_columns(batch, &[("s", DataType::Utf8), ("n", DataType::Utf8)]).is_err(),
        "a type drift is a shape error"
    );
    assert!(
        batch::check_columns(batch, &[("s", DataType::Utf8), ("m", DataType::Int64)]).is_err(),
        "a name drift is a shape error"
    );
}

#[test]
fn the_generated_member_keys_row_converts_and_serializes() {
    let conn = duckdb::Connection::open_in_memory().expect("open duckdb");

    let batches = query_batches(
        &conn,
        "SELECT * FROM (VALUES ('k1'), ('k2')) t(record_key) ORDER BY record_key",
    );
    let rows = er_entities::entity_membership::read::MemberKeysRow::from_batches(&batches)
        .expect("typed rows");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].record_key, "k1");
    assert_eq!(rows[1].record_key, "k2");
    // serde renders the row as the JSON object the HTTP layer serves.
    assert_eq!(
        serde_json::to_value(&rows[0]).expect("serializes"),
        json!({"record_key": "k1"})
    );

    // The declared non-null column holds the line at runtime.
    let null_batches = query_batches(&conn, "SELECT NULL::VARCHAR AS record_key");
    let err = er_entities::entity_membership::read::MemberKeysRow::from_batches(&null_batches)
        .expect_err("NULL in a non-null column");
    assert!(err.to_string().contains("NULL in a non-null column"));

    // A shape drift (wrong type) is an Err from the compiled-in expected schema.
    let drift_batches = query_batches(&conn, "SELECT 1::BIGINT AS record_key");
    assert!(
        er_entities::entity_membership::read::MemberKeysRow::from_batches(&drift_batches).is_err()
    );
}
