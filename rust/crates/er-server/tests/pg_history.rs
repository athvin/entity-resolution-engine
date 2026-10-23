//! The history companions (behind `ER_TEST_PG_DSN`): every INSERT/UPDATE/DELETE
//! on a `history: true` entity leaves a jsonb row image in
//! `<relation>_history` with the acting principal (`er.actor`, session_user
//! fallback), the trigger survives schema evolution (whole-row images name no
//! columns), and a double ensure_schema is idempotent. Run with e.g.
//!   ER_TEST_PG_DSN='host=localhost port=5455 user=er password=er dbname=er' \
//!     cargo test -p er-server --test pg_history

use er_server::pg;

async fn history_rows(
    client: &tokio_postgres::Client,
    row_id: &str,
) -> Vec<(
    String,
    String,
    Option<serde_json::Value>,
    Option<serde_json::Value>,
)> {
    client
        .query(
            "SELECT op, actor, old_row, new_row FROM public.webhooks_history \
             WHERE row_id = $1 ORDER BY history_id",
            &[&row_id],
        )
        .await
        .expect("read history")
        .iter()
        .map(|r| (r.get(0), r.get(1), r.get(2), r.get(3)))
        .collect()
}

#[tokio::test]
async fn history_companions_capture_row_images_with_the_actor() {
    let Ok(dsn) = std::env::var("ER_TEST_PG_DSN") else {
        eprintln!("skipping history test (set ER_TEST_PG_DSN to run)");
        return;
    };

    let mut client = pg::connect(&dsn).await.expect("connect");
    pg::ensure_schema(&mut client).await.expect("ensure schema");
    // Idempotency: replaying the baseline (companion + CREATE OR REPLACE
    // TRIGGER) must succeed on an already-provisioned database.
    pg::ensure_schema(&mut client).await.expect("re-ensure");

    let org_id = er_ids::new_v7();
    pg::ensure_org(&client, "history-org", "/configs/history.yaml")
        .await
        .expect("org");
    let org_id = pg::org_id_by_name(&client, "history-org")
        .await
        .expect("org lookup")
        .unwrap_or(org_id);

    // INSERT under a declared actor.
    pg::set_actor(&client, "history-test@example")
        .await
        .expect("set actor");
    let webhook_id = pg::create_webhook(
        &client,
        &org_id,
        "https://example.test/hook",
        &serde_json::json!(["job.succeeded"]),
        "s3cr3t",
    )
    .await
    .expect("create webhook");

    // UPDATE through the wrapper. `delete_webhook` is a SOFT delete — it sets
    // `active = false`, because `webhook_deliveries` references `webhooks.id`
    // and a hard DELETE could not run once a subscription had delivered
    // anything. Every read filters `AND active`, so this is deletion's
    // observable semantics, and the history companion sees an update image.
    //
    // This used to drive the UPDATE by hand, under a comment reading "webhooks
    // only create/delete" — true when it was written, and the reason the
    // wrapper's own behaviour was never exercised here.
    let deleted = pg::delete_webhook(&client, &org_id, &webhook_id)
        .await
        .expect("retire webhook");
    assert!(deleted, "the first retire must report a row changed");

    // DELETE: driven directly, because nothing in the product hard-deletes a
    // webhook any more and the delete IMAGE still needs covering. Possible
    // here only because this subscription never delivered — which is precisely
    // the foreign key the soft delete exists for.
    client
        .execute("DELETE FROM public.webhooks WHERE id = $1", &[&webhook_id])
        .await
        .expect("hard delete for the image");

    let rows = history_rows(&client, &webhook_id).await;
    let shapes: Vec<(&str, &str)> = rows
        .iter()
        .map(|(op, actor, _, _)| (op.as_str(), actor.as_str()))
        .collect();
    assert_eq!(
        shapes,
        vec![
            ("insert", "history-test@example"),
            ("update", "history-test@example"),
            ("delete", "history-test@example"),
        ],
        "three images in order, all under the declared actor"
    );
    // Image shape: insert has new only, delete has old only, update has both;
    // the images carry the row's columns as jsonb.
    assert!(rows[0].2.is_none() && rows[0].3.is_some(), "insert image");
    assert!(rows[1].2.is_some() && rows[1].3.is_some(), "update images");
    assert!(rows[2].2.is_some() && rows[2].3.is_none(), "delete image");
    assert_eq!(
        rows[0]
            .3
            .as_ref()
            .and_then(|v| v.get("url"))
            .and_then(|v| v.as_str()),
        Some("https://example.test/hook"),
        "the new image carries the inserted values"
    );
    assert_eq!(
        rows[1]
            .3
            .as_ref()
            .and_then(|v| v.get("active"))
            .and_then(serde_json::Value::as_bool),
        Some(false),
        "the update's new image reflects the change"
    );

    // Schema evolution: add a column, write, drop it — the trigger keeps
    // working (it names no columns), and the earlier images keep the old shape.
    client
        .batch_execute("ALTER TABLE public.webhooks ADD COLUMN zz_probe text")
        .await
        .expect("add column");
    let evolved_id = pg::create_webhook(
        &client,
        &org_id,
        "https://example.test/evolved",
        &serde_json::json!([]),
        "s3cr3t-2",
    )
    .await
    .expect("create under evolved schema");
    let evolved = history_rows(&client, &evolved_id).await;
    assert!(
        evolved[0]
            .3
            .as_ref()
            .is_some_and(|v| v.get("zz_probe").is_some()),
        "the evolved image carries the added column"
    );
    client
        .batch_execute("ALTER TABLE public.webhooks DROP COLUMN zz_probe")
        .await
        .expect("drop column");
    let _ = pg::delete_webhook(&client, &org_id, &evolved_id)
        .await
        .expect("delete under restored schema");

    // Fallback: a connection with no actor records the session user.
    let other = pg::connect(&dsn).await.expect("second connect");
    let anon_id = pg::create_webhook(
        &other,
        &org_id,
        "https://example.test/anon",
        &serde_json::json!([]),
        "s3cr3t-3",
    )
    .await
    .expect("anonymous create");
    let anon = history_rows(&client, &anon_id).await;
    assert!(
        !anon[0].1.is_empty() && anon[0].1 != "history-test@example",
        "falls back to session user: {}",
        anon[0].1
    );

    // Tidy the probe rows (the org + its history stay; reruns reuse them).
    let _ = pg::delete_webhook(&other, &org_id, &anon_id).await;
    let _ = client
        .execute(
            "DELETE FROM public.webhooks_history WHERE row_id IN ($1, $2, $3)",
            &[&webhook_id, &evolved_id, &anon_id],
        )
        .await;
}
