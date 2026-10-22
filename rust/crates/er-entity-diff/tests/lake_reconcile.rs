//! The lake reconciler over a real DuckLake (behind `DEDUPE_E2E_DUCKLAKE=1`,
//! which needs the ducklake extension): a fresh lake gets every DDL-owned
//! relation and a ledger; a second pass writes nothing; a stage-materialised
//! relation with a foreign shape is left alone; a dropped nullable column comes
//! back; a retyped column is refused. Run with:
//!   DEDUPE_E2E_DUCKLAKE=1 cargo test -p er-entity-diff --test lake_reconcile

use er_entity_diff::{ReconcileError, attach_lake, reconcile_lake};
use er_entity_spec::Store;

fn enabled() -> bool {
    std::env::var("DEDUPE_E2E_DUCKLAKE").as_deref() == Ok("1")
}

fn count(conn: &duckdb::Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap()
}

#[test]
fn a_tenant_lake_converges_and_its_ledger_records_it() {
    if !enabled() {
        eprintln!("skipping lake reconcile test (set DEDUPE_E2E_DUCKLAKE=1 to run)");
        return;
    }
    let tmp = std::env::temp_dir().join(format!("er_lake_reconcile_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(tmp.join("data")).unwrap();
    let conn = duckdb::Connection::open_in_memory().unwrap();
    attach_lake(
        &conn,
        tmp.join("catalog.ducklake").to_str().unwrap(),
        Some(tmp.join("data").to_str().unwrap()),
        false,
    )
    .unwrap();
    let catalog = er_entities::catalog::catalog();
    let hash = er_entity_diff::catalog_hash(&catalog, Store::Duckdb);

    // ---- phase 1: a fresh lake gets every DDL-owned relation ---------------
    let first = reconcile_lake(&conn, &catalog, "lake_reconcile-test").unwrap();
    assert_eq!(first.catalog_hash, hash);
    let owned = er_entity_diff::lake_relations(&catalog);
    let created: std::collections::BTreeSet<&str> =
        first.applied.iter().map(|p| p.relation.as_str()).collect();
    for rel in &owned {
        assert!(
            created.contains(format!("lake.main.{rel}").as_str()),
            "{rel} was not created: {created:?}"
        );
    }
    let stage = er_schema::model::stage_owned();
    for rel in &stage {
        assert!(
            !created.contains(format!("lake.main.{rel}").as_str()),
            "stage relation {rel} must not be created up front"
        );
    }
    let rows = count(&conn, "SELECT count(*) FROM lake.main.schema_migrations");
    assert_eq!(
        rows as usize,
        first.applied.len(),
        "one ledger row per statement"
    );
    let snapshot_recorded = count(
        &conn,
        "SELECT count(*) FROM lake.main.schema_migrations WHERE snapshot_id IS NOT NULL",
    );
    assert_eq!(
        snapshot_recorded, rows,
        "every row carries the apply snapshot"
    );

    // ---- phase 2: a second pass is a no-op ---------------------------------
    let second = reconcile_lake(&conn, &catalog, "lake_reconcile-test").unwrap();
    assert!(second.applied.is_empty());
    assert!(second.ledger_was_current);
    assert_eq!(
        count(&conn, "SELECT count(*) FROM lake.main.schema_migrations"),
        rows
    );

    // ---- phase 3: a stage relation with a foreign shape is left alone ------
    conn.execute_batch(
        "CREATE OR REPLACE TABLE lake.main.int_std_records AS SELECT 1 AS whatever;",
    )
    .unwrap();
    let third = reconcile_lake(&conn, &catalog, "lake_reconcile-test").unwrap();
    assert!(third.applied.is_empty(), "{:?}", third.applied);
    assert!(third.ahead.is_empty(), "{:?}", third.ahead);

    // ---- phase 4: a dropped nullable column comes back ----------------------
    conn.execute_batch("ALTER TABLE lake.main.runs DROP COLUMN rebuild_reason;")
        .unwrap();
    let fourth = reconcile_lake(&conn, &catalog, "lake_reconcile-test").unwrap();
    assert_eq!(fourth.applied.len(), 1, "{:?}", fourth.applied);
    assert_eq!(fourth.applied[0].relation, "lake.main.runs");
    assert!(
        fourth.applied[0]
            .sql
            .contains("ADD COLUMN rebuild_reason VARCHAR")
    );
    assert_eq!(
        count(
            &conn,
            "SELECT count(*) FROM information_schema.columns WHERE table_catalog = 'lake' \
             AND table_name = 'runs' AND column_name = 'rebuild_reason'"
        ),
        1
    );

    // ---- phase 5: a retyped column is refused, nothing written -------------
    conn.execute_batch("ALTER TABLE lake.main.ingest_batches ALTER COLUMN new_count TYPE DOUBLE;")
        .unwrap();
    let before = count(&conn, "SELECT count(*) FROM lake.main.schema_migrations");
    let err = reconcile_lake(&conn, &catalog, "lake_reconcile-test").unwrap_err();
    match &err {
        ReconcileError::Breaking(items) => assert!(
            items.iter().any(
                |d| d.relation == "lake.main.ingest_batches" && d.message.contains("new_count")
            ),
            "{items:?}"
        ),
        other => panic!("expected a breaking refusal, got {other}"),
    }
    assert_eq!(
        count(&conn, "SELECT count(*) FROM lake.main.schema_migrations"),
        before
    );
    let _ = std::fs::remove_dir_all(&tmp);
}
