//! The default-dataset convergence sweep (behind `ER_TEST_PG_DSN`): every org
//! gains a dataset named `default` whose env, config pointer, delivery root,
//! auto-ingest fields and active version are the org row's **verbatim**;
//! `entity_type` is read from the active published config; the sweep runs on
//! every boot, so an org created after one boot is converged by the next; and
//! re-running never duplicates or overwrites. Run with e.g.
//!   ER_TEST_PG_DSN='host=localhost port=5433 user=er password=er dbname=er' \
//!     cargo test -p er-server --test main pg_dataset_sweep
//!
//! ONE test with sequential phases (the file-level rule `pg_serial.rs` checks).

use er_server::pg;

use crate::common;

const ACCOUNT_YAML: &str = include_str!("../../../../fixtures/static/acct_base/config.yaml");

#[tokio::test]
async fn every_org_converges_to_a_default_dataset() {
    let Some(db) = common::fresh_db("dssweep").await else {
        return;
    };
    let dsn = db.dsn().to_string();
    let mut client = pg::connect(&dsn).await.unwrap();
    pg::ensure_schema(&mut client).await.unwrap();

    // ---- phase 1: an org created AFTER boot is converged by the next boot --
    pg::upsert_org_byo(
        &client,
        "acme",
        "/configs/acme.yaml",
        &serde_json::json!({
            "ER_CONFIG_PATH": "/configs/acme.yaml",
            "ER_LAKE_CATALOG": "/lake/acme/catalog.ducklake",
            "ER_LAKE_DATA_PATH": "/lake/acme/data",
            "ER_TF_PATH": "/lake/acme/tf.csv",
        }),
        Some("/drop/acme"),
        Some("ingest-bucket"),
        Some("tenants/acme"),
    )
    .await
    .unwrap();
    let org_id = pg::org_id_by_name(&client, "acme").await.unwrap().unwrap();
    assert!(
        pg::get_dataset(&client, &org_id, "default")
            .await
            .unwrap()
            .is_none(),
        "no sweep has run since the org appeared"
    );

    pg::ensure_schema(&mut client).await.unwrap(); // the next boot
    let ds = pg::get_dataset(&client, &org_id, "default")
        .await
        .unwrap()
        .expect("the sweep converged the org");
    assert_eq!(ds.entity_type, "person", "no published config: the default");
    assert_eq!(ds.state, "active");
    assert_eq!(ds.config_path, "/configs/acme.yaml");
    assert_eq!(ds.drop_root.as_deref(), Some("/drop/acme"));
    assert_eq!(ds.s3_ingest_bucket.as_deref(), Some("ingest-bucket"));
    assert_eq!(ds.s3_ingest_prefix.as_deref(), Some("tenants/acme"));
    assert_eq!(ds.active_config_version, None);
    assert_eq!(
        ds.env.get("ER_LAKE_CATALOG").and_then(|v| v.as_str()),
        Some("/lake/acme/catalog.ducklake"),
        "the env rides verbatim — nothing moves"
    );
    assert_eq!(
        ds.env.get("ER_TF_PATH").and_then(|v| v.as_str()),
        Some("/lake/acme/tf.csv")
    );

    // ---- phase 2: re-running neither duplicates nor overwrites -------------
    pg::set_dataset_display_name(&client, &org_id, "default", Some("Main"))
        .await
        .unwrap();
    pg::ensure_schema(&mut client).await.unwrap();
    let all = pg::list_datasets(&client, &org_id).await.unwrap();
    assert_eq!(all.len(), 1, "converged, not duplicated");
    assert_eq!(
        all[0].display_name.as_deref(),
        Some("Main"),
        "ON CONFLICT DO NOTHING: an existing row is left exactly as it is"
    );

    // ---- phase 3: entity_type follows the active published config ----------
    // The account fixture config is the self-checking input: assert it parses
    // as an account config FIRST, so a fixture drift fails here by name rather
    // than as a silently-person dataset below.
    let parsed = er_config::load_config_str(ACCOUNT_YAML).expect("fixture config parses");
    assert!(
        matches!(parsed.entity_type, er_config::EntityType::Account),
        "fixtures/static/acct_base/config.yaml is the account fixture"
    );
    pg::upsert_org_byo(
        &client,
        "orgs-inc",
        "/configs/orgs-inc.yaml",
        &serde_json::json!({ "ER_CONFIG_PATH": "/configs/orgs-inc.yaml" }),
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let orgs_inc = pg::org_id_by_name(&client, "orgs-inc")
        .await
        .unwrap()
        .unwrap();
    // A pre-dataset row, written as raw SQL on purpose. Reading
    // `entity_type` from a published config is something the converge only
    // ever has to do for an org that has a config and NO dataset, and since
    // `0004` made `dataset_id` required that shape can no longer be built
    // through the generated writers — which is the point: it is the shape an
    // UPGRADING deployment arrives in, not one a current store can reach.
    client
        .batch_execute(
            "ALTER TABLE public.config_versions ALTER COLUMN dataset_id DROP NOT NULL;
             DELETE FROM public.fixups WHERE name = '0004_dataset_id_required';",
        )
        .await
        .unwrap();
    let version: i64 = 1;
    client
        .execute(
            "INSERT INTO public.config_versions
               (id, org_id, dataset_id, version, yaml, config_hash, state, created_by)
             VALUES ($1, $2, NULL, $3, $4, 'deadbeef', 'published', 'pg_dataset_sweep')",
            &[&er_ids::new_v7(), &orgs_inc, &version, &ACCOUNT_YAML],
        )
        .await
        .unwrap();
    er_entities::org::update::set_active_config(&client, version, "orgs-inc")
        .await
        .unwrap();

    pg::ensure_schema(&mut client).await.unwrap();
    let ds = pg::get_dataset(&client, &orgs_inc, "default")
        .await
        .unwrap()
        .expect("swept");
    assert_eq!(ds.entity_type, "account", "read from the published config");
    assert_eq!(ds.active_config_version, Some(version));

    // ---- phase 4: the 0002 backfill stamps pre-dataset rows -----------------
    // The fixup ran (and was ledgered) on this database's very first
    // ensure_schema, when there was nothing to stamp — which is exactly what
    // happens on a fresh deployment. A row created WITHOUT a dataset_id (as
    // every row predating the column was) is healed by the backfill statement
    // itself, which is what the fixup replays on a pre-dataset database.
    let ledgered = er_entities::fixup::read::by_name(&client, "0002_backfill_dataset_ids")
        .await
        .unwrap();
    assert_eq!(
        ledgered.len(),
        1,
        "the backfill fixup is ledgered semantics-once"
    );

    let acme = pg::org_id_by_name(&client, "acme").await.unwrap().unwrap();
    let orphan = pg::enqueue(
        &client,
        &acme,
        None,
        "run_all_full",
        0,
        None,
        None,
        pg::Provenance::default(),
    )
    .await
    .unwrap();
    assert!(
        orphan.dataset_id.is_none(),
        "created as a pre-dataset row would be"
    );
    pg::backfill_dataset_ids(&mut client).await.unwrap();
    let healed = er_entities::job::read::by_id(&client, &acme, &orphan.id)
        .await
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    let default_id = pg::default_dataset_id(&client, &acme)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        healed.dataset_id.as_deref(),
        Some(default_id.as_str()),
        "the backfill stamps a NULL row with its org's default dataset"
    );
}
