//! The per-dataset lake-isolation seam (behind `ER_TEST_PG_DSN`): two DuckLake
//! catalogs sharing one Postgres metadata database, separated only by the
//! `METADATA_SCHEMA` attach option (`ER_LAKE_CATALOG_SCHEMA`), are fully
//! disjoint lakes — a table created in one is invisible to the other — and the
//! schema is created by the first attach. A catalog schema is also pinned to
//! its data path: re-attaching it under a different `DATA_PATH` is refused
//! rather than silently re-pointed, which is the guarantee that makes the
//! schema the isolation unit. Run with:
//!   ER_TEST_PG_DSN='host=localhost port=5432 user=er password=er dbname=er' \
//!     cargo test -p er-server --test main pg_lake_catalog_schema
//!
//! ONE test with sequential phases (the file-level rule `pg_serial.rs` checks).

use crate::common;

/// Open a lake and run one closure against its DuckDB connection, off the
/// async runtime (DuckDB is blocking).
async fn with_lake<T: Send + 'static>(
    metadata: String,
    data_path: String,
    catalog_schema: &str,
    f: impl FnOnce(&duckdb::Connection) -> T + Send + 'static,
) -> T {
    let schema = catalog_schema.to_string();
    tokio::task::spawn_blocking(move || {
        let session = er_cli::open_lake_with(&metadata, &data_path, Some(&schema))
            .unwrap_or_else(|e| panic!("opening the {schema} lake: {e:#}"));
        f(session.connection())
    })
    .await
    .expect("the lake task is not cancelled")
}

#[tokio::test]
async fn ducklake_metadata_schemas_are_disjoint_pinned_lakes() {
    let Some(db) = common::fresh_db("lakeschema").await else {
        return;
    };
    let metadata = format!("postgres:{}", db.dsn());
    let tmp = std::env::temp_dir().join(format!("er_lakeschema_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let data_a = tmp.join("a").to_str().expect("utf-8 temp path").to_string();
    let data_b = tmp.join("b").to_str().expect("utf-8 temp path").to_string();
    std::fs::create_dir_all(&data_a).unwrap();
    std::fs::create_dir_all(&data_b).unwrap();

    // ---- phase 1: the first attach creates the schema; a table goes in A ---
    // `lake.t`, not the fully qualified spelling: the scratch table is not a
    // declared relation, and the declared_names posterity gate holds every
    // three-part `lake.main.*` literal in the workspace to the S5 registry.
    with_lake(metadata.clone(), data_a.clone(), "ds_a", |conn| {
        conn.execute_batch("CREATE TABLE lake.t AS SELECT 'a' AS v;")
            .expect("creating a table in the ds_a lake");
    })
    .await;

    // ---- phase 2: B shares the database, sees none of it, gets its own ----
    with_lake(metadata.clone(), data_b.clone(), "ds_b", |conn| {
        let tables: i64 = conn
            .query_row(
                "SELECT count(*) FROM duckdb_tables() WHERE database_name = 'lake'",
                [],
                |r| r.get(0),
            )
            .expect("listing the ds_b lake's tables");
        assert_eq!(
            tables, 0,
            "the ds_b lake must start empty — a visible table means the two \
             metadata schemas are NOT disjoint catalogs"
        );
        conn.execute_batch("CREATE TABLE lake.t AS SELECT 'b' AS v;")
            .expect("creating a table in the ds_b lake");
    })
    .await;

    // ---- phase 3: A still holds exactly its own row ------------------------
    with_lake(metadata.clone(), data_a.clone(), "ds_a", |conn| {
        let (rows, v): (i64, String) = conn
            .query_row("SELECT count(*), min(v) FROM lake.t", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .expect("reading the ds_a lake back");
        assert_eq!((rows, v.as_str()), (1, "a"), "ds_a sees only its own data");
    })
    .await;

    // ---- phase 4: both schemas exist in Postgres and hold catalog tables ---
    let client = er_server::pg::connect(db.dsn())
        .await
        .expect("connecting to the metadata database");
    let rows = client
        .query(
            "SELECT table_schema FROM information_schema.tables
             WHERE table_schema IN ('ds_a', 'ds_b') AND table_name = 'ducklake_table'
             ORDER BY table_schema",
            &[],
        )
        .await
        .expect("listing the DuckLake catalog schemas");
    let schemas: Vec<&str> = rows.iter().map(|r| r.get(0)).collect();
    assert_eq!(
        schemas,
        ["ds_a", "ds_b"],
        "each attach keeps its catalog tables in its own schema"
    );

    // ---- phase 5: a schema is pinned to its data path ----------------------
    // Re-attaching ds_a under B's data path must be refused. This refusal is
    // what lets a stored env treat (catalog, schema) as naming one lake: a
    // mis-derived data path surfaces as an attach error, not as one dataset
    // silently writing Parquet into another's prefix.
    let err = tokio::task::spawn_blocking({
        let metadata = metadata.clone();
        let data_b = data_b.clone();
        move || er_cli::open_lake_with(&metadata, &data_b, Some("ds_a")).map(|_| ())
    })
    .await
    .expect("the re-attach task is not cancelled");
    let err = err.expect_err(
        "re-attaching metadata schema ds_a under a different DATA_PATH must be \
         refused — silently re-pointing it would break dataset isolation",
    );
    let msg = format!("{err:#}").to_lowercase();
    assert!(
        msg.contains("data path") || msg.contains("data_path"),
        "the refusal should name the data path mismatch, got: {msg}"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}
