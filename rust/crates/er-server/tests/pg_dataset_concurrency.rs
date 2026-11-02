//! The per-dataset writer lock and the per-org concurrency cap (behind
//! `ER_TEST_PG_DSN`): with the shipped cap of 1 an org still runs one job at a
//! time (the pre-dataset profile, unchanged); raising the org's
//! `max_concurrent_jobs` lets two DATASETS run concurrently while each dataset
//! stays strictly single-writer; and an org-level job (dataset_id IS NULL)
//! conflicts with everything of its org, in both directions. Run with e.g.
//!   ER_TEST_PG_DSN='host=localhost port=5433 user=er password=er dbname=er' \
//!     cargo test -p er-server --test main pg_dataset_concurrency
//!
//! ONE test with sequential phases (the file-level rule `pg_serial.rs` checks).

use er_server::pg;

use crate::common;

async fn enqueue(
    client: &tokio_postgres::Client,
    org_id: &str,
    ds: Option<&str>,
    kind: &str,
) -> er_server::Job {
    pg::enqueue(
        client,
        org_id,
        ds,
        kind,
        0,
        None,
        None,
        pg::Provenance::default(),
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn datasets_run_concurrently_under_the_org_cap() {
    let Some(db) = common::fresh_db("dsconc").await else {
        return;
    };
    let dsn = db.dsn().to_string();
    let mut client = pg::connect(&dsn).await.unwrap();
    pg::ensure_schema(&mut client).await.unwrap();

    pg::upsert_org_byo(
        &client,
        "acme",
        "/configs/acme.yaml",
        &serde_json::json!({ "ER_CONFIG_PATH": "/configs/acme.yaml" }),
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let org_id = pg::org_id_by_name(&client, "acme").await.unwrap().unwrap();
    // The next boot's convergence sweep gives acme its default dataset…
    pg::ensure_schema(&mut client).await.unwrap();
    let default_ds = pg::default_dataset_id(&client, &org_id)
        .await
        .unwrap()
        .unwrap();
    // …and a second dataset arrives BYO.
    assert!(
        pg::create_dataset_byo(
            &client,
            &org_id,
            "crm",
            None,
            "person",
            "/configs/acme/crm.yaml",
            &serde_json::json!({ "ER_CONFIG_PATH": "/configs/acme/crm.yaml" }),
            None,
            None,
            None,
        )
        .await
        .unwrap()
    );
    let crm_ds = pg::get_dataset(&client, &org_id, "crm")
        .await
        .unwrap()
        .unwrap()
        .id;

    // ---- phase 1: the shipped cap of 1 keeps the pre-dataset profile --------
    let a = enqueue(&client, &org_id, Some(&default_ds), "run_all_full").await;
    let crm_run = enqueue(&client, &org_id, Some(&crm_ds), "run_all_full").await;
    let claimed = pg::claim(&client, 1)
        .await
        .unwrap()
        .expect("one job claims");
    assert_eq!(claimed.id, a.id, "priority order: the first enqueued");
    assert!(
        pg::claim(&client, 1).await.unwrap().is_none(),
        "cap 1: the second dataset's job waits, exactly as before datasets"
    );

    // ---- phase 2: raising the org cap unlocks per-dataset concurrency ------
    // Test scratch: the cap column has no API yet (it arrives with the
    // dataset-scoped routes); the claim statement reads it either way.
    client
        .execute(
            "UPDATE public.orgs SET max_concurrent_jobs = 2 WHERE id = $1",
            &[&org_id],
        )
        .await
        .unwrap();
    let second = pg::claim(&client, 1)
        .await
        .unwrap()
        .expect("two datasets run at once");
    assert_eq!(second.id, crm_run.id, "the waiting crm run is what claims");
    assert_eq!(second.dataset_id.as_deref(), Some(crm_ds.as_str()));

    // Each dataset stays strictly single-writer: a third job on a busy
    // dataset does not claim even with org capacity to spare.
    client
        .execute(
            "UPDATE public.orgs SET max_concurrent_jobs = 10 WHERE id = $1",
            &[&org_id],
        )
        .await
        .unwrap();
    let crm_correct = enqueue(&client, &org_id, Some(&crm_ds), "correct").await;
    assert!(
        pg::claim(&client, 1).await.unwrap().is_none(),
        "the crm dataset already has an active job"
    );

    // ---- phase 3: an org-level job conflicts with everything ---------------
    let d = enqueue(&client, &org_id, None, "provision").await;
    assert!(
        pg::claim(&client, 1).await.unwrap().is_none(),
        "an org-level job must not start beside any of the org's active jobs"
    );
    pg::mark(&client, &claimed.id, "succeeded").await.unwrap();
    assert!(
        pg::claim(&client, 1).await.unwrap().is_none(),
        "…nor while the other dataset is still running"
    );
    pg::mark(&client, &second.id, "succeeded").await.unwrap();
    // The org-level job still waits: crm's `correct`, queued ahead of it,
    // claims first and keeps blocking it while active.
    let c_claimed = pg::claim(&client, 1)
        .await
        .unwrap()
        .expect("crm's correct claims next");
    assert_eq!(c_claimed.id, crm_correct.id);
    pg::mark(&client, &c_claimed.id, "succeeded").await.unwrap();
    let org_level = pg::claim(&client, 1)
        .await
        .unwrap()
        .expect("the org-level job claims once the org is quiet");
    assert_eq!(org_level.id, d.id);
    // And in the other direction: nothing of the org's claims beside it.
    let _e = enqueue(&client, &org_id, Some(&crm_ds), "run_all_incremental").await;
    assert!(
        pg::claim(&client, 1).await.unwrap().is_none(),
        "the org-level job blocks the crm job queued behind it"
    );
}
