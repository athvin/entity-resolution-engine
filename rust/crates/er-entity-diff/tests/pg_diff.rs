//! The Postgres differ against a live database (gated on `ER_TEST_PG_DSN`): create
//! a table from the entity's shape, read it back, and confirm the diff is empty;
//! then alter a column type and confirm the differ reports a breaking mismatch.

use std::path::PathBuf;

use er_entity_diff::{diff_store, read_postgres_columns};
use er_entity_spec::{Catalog, Store};
use tokio_postgres::NoTls;

fn entities_dir() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../../entities"))
}

#[tokio::test]
async fn postgres_diff_matches_then_detects_a_type_change() {
    let Ok(dsn) = std::env::var("ER_TEST_PG_DSN") else {
        eprintln!("skipping: ER_TEST_PG_DSN unset");
        return;
    };
    let (client, conn) = tokio_postgres::connect(&dsn, NoTls).await.unwrap();
    tokio::spawn(async move {
        let _ = conn.await;
    });

    // A clean `jobs`-shaped table matching the job entity's generated Postgres DDL
    // (job columns are bigint).
    client
        .batch_execute(
            "DROP TABLE IF EXISTS jobs CASCADE; \
             CREATE TABLE jobs ( \
               id text NOT NULL, org_id text NOT NULL, kind text NOT NULL, \
               params jsonb NOT NULL DEFAULT '{}'::jsonb, state text NOT NULL DEFAULT 'queued', \
               priority bigint NOT NULL DEFAULT 0, idempotency_key text, run_id text, \
               attempt bigint NOT NULL DEFAULT 0, max_attempts bigint NOT NULL DEFAULT 3, \
               error_class text, error_detail text, not_before timestamptz, created_by text, schedule_id text, resource_class text, \
               trace_context text, created_at timestamptz NOT NULL DEFAULT now(), \
               updated_at timestamptz NOT NULL DEFAULT now(), started_at timestamptz, \
               finished_at timestamptz, PRIMARY KEY (id));",
        )
        .await
        .unwrap();

    let catalog = Catalog::load_dir(entities_dir()).unwrap();
    let relations: Vec<String> = catalog
        .for_store(Store::Postgres)
        .iter()
        .map(|e| e.relation.clone())
        .collect();

    let live = read_postgres_columns(&client, &relations).await.unwrap();
    let job_live = live.get("jobs").expect("jobs read back");
    assert_eq!(job_live.len(), 21, "21 columns read");

    // Diff just the job entity against the live `jobs`: no difference.
    let job_only = Catalog::from_entities(vec![catalog.by_name("job").unwrap().clone()]).unwrap();
    let diffs = diff_store(&job_only, Store::Postgres, &live);
    assert_eq!(diffs, Vec::new(), "matching schema should not diff");

    // Narrow a column and confirm the differ flags a breaking type change.
    client
        .batch_execute("ALTER TABLE jobs ALTER COLUMN priority TYPE integer;")
        .await
        .unwrap();
    let live2 = read_postgres_columns(&client, &relations).await.unwrap();
    let diffs2 = diff_store(&job_only, Store::Postgres, &live2);
    assert!(
        diffs2
            .iter()
            .any(|d| d.message.contains("priority: type integer -> bigint")),
        "expected a priority type diff, got {diffs2:?}"
    );

    client.batch_execute("DROP TABLE jobs CASCADE;").await.ok();
}
