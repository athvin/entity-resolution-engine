//! Two datasets of one org do not see each other's control-plane rows
//! (behind `ER_TEST_PG_DSN`).
//!
//! PR6 moved every route under its dataset, but several statements still
//! filtered on `org_id` alone, so the config list, the model registry and the
//! label set were shared by every dataset in the org — a tenant with a
//! `people` and an `accounts` dataset saw one combined history in both. The
//! routes said "dataset"; the SQL said "org". This pins the SQL.
//!
//! Deliberately asserts the ABSENCE of the sibling's rows rather than just
//! the presence of its own: a statement that returns everything passes a
//! presence-only check.
//!
//! ONE test with sequential phases (the file-level rule `pg_serial.rs` checks).

use er_server::pg;

use crate::common;

#[tokio::test]
async fn one_orgs_datasets_do_not_see_each_others_rows() {
    let Some(db) = common::fresh_db("dsisolation").await else {
        return;
    };
    let dsn = db.dsn().to_string();
    let mut client = pg::connect(&dsn).await.unwrap();
    pg::ensure_schema(&mut client).await.unwrap();

    // One org, two datasets — the shape the whole program exists to support.
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
    pg::converge_default_datasets(&mut client).await.unwrap();
    let org_id = pg::org_id_by_name(&client, "acme")
        .await
        .unwrap()
        .expect("acme exists");
    let people = pg::get_dataset(&client, &org_id, "default")
        .await
        .unwrap()
        .expect("the converged default")
        .id;
    assert!(
        pg::create_dataset_byo(
            &client,
            &org_id,
            "accounts",
            None,
            "account",
            "/configs/acme/accounts.yaml",
            &serde_json::json!({ "ER_CONFIG_PATH": "/configs/acme/accounts.yaml" }),
            None,
            None,
            None,
        )
        .await
        .unwrap()
    );
    let accounts = pg::get_dataset(&client, &org_id, "accounts")
        .await
        .unwrap()
        .expect("just created")
        .id;

    // ---- config versions ---------------------------------------------------
    //
    // Two drafts in `people`, one in `accounts`. Version NUMBERING stays
    // org-wide until the natural key flips (the live `UNIQUE (org_id,
    // version)` forbids restarting at 1 per dataset), so this asserts on
    // which rows each dataset SEES, not on what they are numbered.
    for yaml in ["a: 1", "a: 2"] {
        pg::create_config_version(&client, &org_id, Some(&people), yaml, "hash", "tester")
            .await
            .unwrap();
    }
    let only_accounts =
        pg::create_config_version(&client, &org_id, Some(&accounts), "b: 1", "hash", "tester")
            .await
            .unwrap();

    let people_versions = pg::list_config_versions(&client, &people).await.unwrap();
    let account_versions = pg::list_config_versions(&client, &accounts).await.unwrap();
    assert_eq!(
        people_versions.len(),
        2,
        "the people dataset sees its own two drafts, not three"
    );
    assert_eq!(account_versions.len(), 1, "and not the sibling's two");
    assert_eq!(
        account_versions[0].version, only_accounts,
        "the one it sees is its own"
    );
    assert!(
        pg::get_config_version(&client, &accounts, people_versions[0].version)
            .await
            .unwrap()
            .is_none(),
        "a version belonging to the sibling is not readable through this dataset"
    );

    // ---- the label set -----------------------------------------------------
    //
    // Labels train the model, so leakage here would have one dataset's
    // verdicts teaching the other's matcher.
    pg::record_reviewer_label(
        &client,
        &org_id,
        Some(&people),
        ("crm:A", "crm:B"),
        "match",
        "tester",
    )
    .await
    .unwrap();
    let people_labels = er_entities::pair_label::read::all(&client, &people)
        .await
        .unwrap();
    let account_labels = er_entities::pair_label::read::all(&client, &accounts)
        .await
        .unwrap();
    assert_eq!(people_labels.len(), 1, "{people_labels:?}");
    assert!(
        account_labels.is_empty(),
        "the sibling's verdicts must not train this dataset: {account_labels:?}"
    );
}
