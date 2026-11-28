//! #838: the merge-plans read's `affected` set, executed against real lake
//! tables in an in-memory DuckDB (the generator is plan-vector tested in
//! er-sql-check; this runs it on rows).
//!
//! - A steward/assertion merge: the absorbed entity carries the `merged`
//!   event and no members; the survivor earns no `member_added` because every
//!   record was already clustered. The survivor must still have a plan.
//! - A split: the receiver carries the `split` event, the donor is named in
//!   `split_from`. Both changed, so both must have a plan.

use std::path::PathBuf;

fn ddl(table: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../er-entities/ddl/lake")
        .join(format!("{table}.sql"));
    std::fs::read_to_string(path).expect("lake DDL")
}

fn plans(conn: &duckdb::Connection) -> Vec<(String, String, i64)> {
    let sql = er_golden::merge_plans_sql(
        // `most_complete` elects from membership and standardized records;
        // `affected` is the same CTE for every policy.
        "most_complete",
        "lake.main",
        None,
        false,
        false,
    )
    .unwrap();
    let mut stmt = conn.prepare(&sql).unwrap();
    let rows = stmt
        .query_map(duckdb::params![100_i64], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
            ))
        })
        .unwrap();
    let mut out: Vec<(String, String, i64)> = rows.map(Result::unwrap).collect();
    out.sort();
    out.dedup();
    out
}

#[test]
fn a_merge_survivor_and_both_sides_of_a_split_have_plans() {
    let conn = duckdb::Connection::open_in_memory().unwrap();
    conn.execute_batch("ATTACH ':memory:' AS lake;").unwrap();
    for table in [
        "entity_events",
        "entity_membership",
        "golden_lineage",
        "golden_records",
        "int_std_records",
    ] {
        conn.execute_batch(&ddl(table)).unwrap();
    }
    let member = |entity: &str, key: &str| {
        let (source, id) = key.split_once(':').unwrap();
        conn.execute(
            "INSERT INTO lake.main.entity_membership (id, source_system, source_record_id, record_key, entity_id, assigned_at, run_id, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, now(), 'run-1', now(), now())",
            duckdb::params![format!("m-{key}"), source, id, key, entity],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO lake.main.int_std_records (id, record_key, source_system, source_record_id, content_hash, std_version, name_variants, ingest_batch_id, ingested_at, created_at, updated_at, email) \
             VALUES (?, ?, ?, ?, 'h', 'v1', [], 'run-1', now(), now(), now(), ?)",
            duckdb::params![format!("s-{key}"), key, source, id, format!("{id}@x.test")],
        )
        .unwrap();
    };
    let event = |entity: &str, kind: &str, details: &str| {
        conn.execute(
            "INSERT INTO lake.main.entity_events (id, seq, run_id, entity_id, event_type, details, details_hash, occurred_at, created_at, updated_at) \
             VALUES (?, 0, 'run-2', ?, ?, ?::JSON, 'h', now(), now(), now())",
            duckdb::params![format!("e-{entity}-{kind}"), entity, kind, details],
        )
        .unwrap();
    };
    // Merge: E-absorbed folded into E-survivor; the survivor's members were
    // all clustered already, so only the absorbed entity has an event.
    member("E-survivor", "crm:S1");
    member("E-survivor", "crm:S2");
    member("E-survivor", "web:S3");
    event(
        "E-absorbed",
        "merged",
        r#"{"member_keys": ["web:S3"], "merged_into": "E-survivor"}"#,
    );
    // Split: web:D3, web:D4 left E-donor for E-receiver.
    member("E-donor", "crm:D1");
    member("E-donor", "crm:D2");
    member("E-receiver", "web:D3");
    member("E-receiver", "web:D4");
    event(
        "E-receiver",
        "split",
        r#"{"member_keys": ["web:D3", "web:D4"], "split_from": "E-donor"}"#,
    );
    // An untouched entity stays out.
    member("E-quiet", "crm:Q1");
    member("E-quiet", "crm:Q2");

    let got: Vec<String> = plans(&conn)
        .into_iter()
        .map(|(e, _, _)| e)
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    assert_eq!(got, ["E-donor", "E-receiver", "E-survivor"], "{got:?}");
}
