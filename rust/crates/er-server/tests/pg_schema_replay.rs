//! `ensure_schema` against a LIVE OLDER store (behind `ER_TEST_PG_DSN`): the
//! baseline replay carries indexes over columns the reconciler has not added
//! yet, and on a store from before those columns existed the CREATE must be
//! deferred past the reconcile rather than failing the boot. Found the hard
//! way: the dev stack's control-plane volume predated the dataset columns and
//! `er-server migrate` died with `column "dataset_id" does not exist` — a
//! fresh database can never reproduce this, because there the baseline creates
//! every table already carrying its full column set. Run with:
//!   ER_TEST_PG_DSN='host=localhost port=5433 user=er password=er dbname=er' \
//!     cargo test -p er-server --test main pg_schema_replay
//!
//! ONE test with sequential phases (the file-level rule `pg_serial.rs` checks).

use er_server::pg;

use crate::common;

#[tokio::test]
async fn a_live_older_store_boots_through_the_deferred_replay() {
    let Some(db) = common::fresh_db("schemareplay").await else {
        return;
    };
    let dsn = db.dsn().to_string();
    let mut client = pg::connect(&dsn).await.unwrap();
    pg::ensure_schema(&mut client).await.unwrap();

    // An org that predates datasets, so the re-boot also re-proves the
    // convergence sweep against the aged store.
    pg::upsert_org_byo(
        &client,
        "elder",
        "/configs/elder.yaml",
        &serde_json::json!({}),
        None,
        None,
        None,
    )
    .await
    .unwrap();

    // ---- phase 1: age the store back to the pre-dataset shape --------------
    //
    // DROP COLUMN takes the dataset-keyed indexes and foreign keys with it;
    // dropping `datasets` afterwards removes the converge's target. The fixup
    // ledger rows go too, so the aged store re-runs the backfill and the lock
    // swap exactly as a real older deployment would.
    let tables: Vec<String> = client
        .query(
            "SELECT table_name FROM information_schema.columns
             WHERE table_schema = 'public' AND column_name = 'dataset_id'",
            &[],
        )
        .await
        .unwrap()
        .iter()
        .map(|r| r.get::<_, String>(0))
        .collect();
    assert!(
        tables.len() >= 10,
        "the catalog should scope well over ten tables by dataset, got {tables:?}"
    );
    for table in &tables {
        client
            .batch_execute(&format!(
                "ALTER TABLE public.\"{table}\" DROP COLUMN dataset_id CASCADE"
            ))
            .await
            .unwrap();
    }
    client
        .batch_execute("DROP TABLE public.datasets CASCADE")
        .await
        .unwrap();
    client
        .batch_execute("DELETE FROM public.fixups")
        .await
        .unwrap();

    // ---- phase 2: the next boot heals the whole distance -------------------
    //
    // Baseline replay (deferring every statement that needs the missing
    // columns), reconcile (adding them back), the deferred statements, both
    // fixups, and the convergence sweep — one call, no hand-holding.
    pg::ensure_schema(&mut client).await.unwrap();

    let healed: i64 = client
        .query_one(
            "SELECT count(*) FROM information_schema.columns
             WHERE table_schema = 'public' AND column_name = 'dataset_id'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(
        healed as usize,
        tables.len(),
        "every aged table regained its dataset_id column"
    );
    let lock_index: bool = client
        .query_one(
            "SELECT to_regclass('public.jobs_one_active_per_dataset') IS NOT NULL",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert!(
        lock_index,
        "the per-dataset writer lock is one of the deferred statements and must come back"
    );
    let converged: i64 = client
        .query_one(
            "SELECT count(*) FROM public.datasets d
             JOIN public.orgs o ON o.id = d.org_id
             WHERE o.name = 'elder' AND d.name = 'default'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(
        converged, 1,
        "the aged org converged to its default dataset"
    );

    // ---- phase 3: an APPLIED contract fixup must not refuse a boot ---------
    //
    // `0004_dataset_id_required` is the registry's first gated `Kind::Contract`:
    // it may only run on a store that recorded the release which stopped
    // addressing these rows by org. Give the guard a history it cannot possibly
    // satisfy — a catalog version no build ever carried — while leaving the
    // ledger row phase 2 wrote in place.
    //
    // Boot must still succeed. Once the ledger says the fixup ran, the removal
    // has already happened, so a refusal could not undo it; it would only
    // strand a store that migrated correctly, permanently and on every
    // subsequent boot. That is why the ledger is consulted before the guard.
    let foreign = "0".repeat(64);
    client
        .execute(
            "INSERT INTO public.schema_migrations (id, catalog_hash, kind, applied_by)
             VALUES ($1, $2, 'verified', 'pg_schema_replay')",
            &[&er_ids::new_v7(), &foreign],
        )
        .await
        .unwrap();
    pg::ensure_schema(&mut client).await.unwrap();

    // ---- phase 4: ...because that history really is unsatisfiable ----------
    //
    // The control for phase 3. Drop only the one ledger row and the same boot
    // now refuses, which is what makes phase 3's success attributable to the
    // ordering rather than to a guard that never fires.
    client
        .execute(
            "DELETE FROM public.fixups WHERE name = $1",
            &[&"0004_dataset_id_required"],
        )
        .await
        .unwrap();
    let err = pg::ensure_schema(&mut client).await.expect_err(
        "an unapplied contract fixup on a store missing its expand release must refuse",
    );
    match err {
        pg::SchemaError::ContractNotReady { fixup, .. } => {
            assert_eq!(fixup, "0004_dataset_id_required");
        }
        other => panic!("expected ContractNotReady, got {other}"),
    }
}
