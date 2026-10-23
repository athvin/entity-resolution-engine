//! `er` reconciles its lake before running (gated on `DEDUPE_E2E_DUCKLAKE=1`).
//!
//! `create_ddl_table` is `CREATE TABLE IF NOT EXISTS`: it creates a missing
//! table and never touches an existing one. The dispatcher compensated by
//! reconciling before every job; the CLI did not, so a lake created before a
//! column was added never gained it.
//!
//! That is not hypothetical — it is what seeding the dev stack produced:
//!
//! ```text
//! run seed-base: screening crm could not be recorded: Binder Error:
//!   Referenced update column matched_count not found in table!
//!   Did you mean: "changed_count"
//! ```
//!
//! E9b added four columns to `ingest_batches`; the lake predated them. And
//! because the screening write is best-effort, the run printed one line and
//! reported success while recording nothing.
//!
//! Run with:
//!   DEDUPE_E2E_DUCKLAKE=1 cargo test -p er-cli --test lake_schema_reconcile

/// The pre-E9b shape of `ingest_batches`: the four screening columns absent,
/// every other column exactly as the catalog has it.
///
/// The four are NULLABLE in the catalog, which is what makes them additive —
/// the reconciler refuses a missing NOT NULL column as BREAKING. An earlier
/// draft of this fixture also omitted `path` and `full_refresh_keys` and got
/// `ERR_SCHEMA_BREAKING` for exactly that reason, which is the contract working.
const OLD_INGEST_BATCHES: &str = "
CREATE TABLE IF NOT EXISTS lake.main.ingest_batches (
  id VARCHAR NOT NULL,
  run_id VARCHAR NOT NULL,
  source_system VARCHAR NOT NULL,
  path VARCHAR NOT NULL,
  new_count BIGINT NOT NULL,
  changed_count BIGINT NOT NULL,
  unchanged_count BIGINT NOT NULL,
  tombstone_count BIGINT NOT NULL,
  resurrected_count BIGINT NOT NULL,
  full_refresh_keys BOOLEAN NOT NULL,
  created_at TIMESTAMP NOT NULL,
  updated_at TIMESTAMP NOT NULL
);";

fn enabled() -> bool {
    std::env::var("DEDUPE_E2E_DUCKLAKE").as_deref() == Ok("1")
}

fn columns(session: &dedupe_core::Session, table: &str) -> Vec<String> {
    session
        .query_strings(&format!(
            "select column_name from duckdb_columns() where table_name = '{table}'"
        ))
        .unwrap()
        .into_iter()
        .map(|r| r[0].clone())
        .collect()
}

#[test]
fn a_lake_older_than_a_column_gains_it() {
    if !enabled() {
        eprintln!("skipping lake reconcile (set DEDUPE_E2E_DUCKLAKE=1 to run)");
        return;
    }
    let tmp = std::env::temp_dir().join(format!("er_reconcile_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(tmp.join("data")).unwrap();
    let catalog = tmp.join("catalog.ducklake").to_string_lossy().into_owned();
    let data = tmp.join("data").to_string_lossy().into_owned();

    let session = er_cli::open_lake(&catalog, &data).unwrap();
    session.execute_batch(OLD_INGEST_BATCHES).unwrap();

    // Precondition: the lake really is older than the column. Without this a
    // pass cannot be told apart from "the table was fine all along".
    let before = columns(&session, "ingest_batches");
    assert!(
        !before.contains(&"matched_count".to_string()),
        "precondition: the fixture lake must predate the column, got {before:?}"
    );
    assert!(
        before.contains(&"changed_count".to_string()),
        "precondition: it is the OLD shape, not an empty one"
    );

    let applied = er_cli::reconcile_lake_schema(&session).unwrap();
    assert!(
        applied > 0,
        "a lake missing four columns must need statements"
    );

    let after = columns(&session, "ingest_batches");
    for column in [
        "matched_count",
        "review_count",
        "unmatched_count",
        "screened_at",
    ] {
        assert!(
            after.contains(&column.to_string()),
            "{column} must exist after reconciling, got {after:?}"
        );
    }
    // ...and nothing was dropped: additive means additive.
    assert!(
        after.contains(&"changed_count".to_string()),
        "reconciling must not remove an existing column"
    );

    // Idempotent: a second pass has nothing to do. A reconciler that keeps
    // applying would rewrite the ledger on every CLI invocation.
    assert_eq!(
        er_cli::reconcile_lake_schema(&session).unwrap(),
        0,
        "reconciling an already-current lake must be a no-op"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}
