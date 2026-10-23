//! The approval gate: a scheduled job merges only what a steward approved
//! (behind `ER_TEST_PG_DSN`).
//!
//! What these assertions pin down:
//!   * **off by default.** A tenant with no `approval.tag` stages nothing, ever.
//!     An approval workflow that is on until switched off would merge groups
//!     nobody looked at;
//!   * the gate decides WHAT to merge and never how — it stages a
//!     `resolve_review` and enqueues the existing `apply_corrections`, so this
//!     adds no second way to merge a group;
//!   * the tag is REMOVED as it is staged, which is what makes a second tick
//!     idempotent rather than a re-stage of everything;
//!   * **retiring the tag disables the workflow**, because the config names a
//!     tag by name and the lookup is of live tags only. That is the behaviour an
//!     operator would expect from retiring it, and the reason the config holds a
//!     name rather than an id.
//!
//! Run with e.g.
//!   ER_TEST_PG_DSN='host=localhost port=5466 user=er password=er dbname=er' \
//!     cargo test -p er-server --test pg_approval

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use er_server::{AppState, app, pg};
use tower::ServiceExt;

const DEFAULT_YAML: &str = include_str!("../../er-config/tests/fixtures/default.yaml");

fn req(method: &str, uri: &str, bearer: &str, body: Body) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {bearer}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(body)
        .unwrap()
}

async fn json_body(resp: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

/// Rows in `staged_steward_actions` for this org, and the queued apply count.
async fn staged_and_queued(client: &tokio_postgres::Client, org_id: &str) -> (i64, i64) {
    let staged: i64 = client
        .query_one(
            "SELECT count(*) FROM public.staged_steward_actions WHERE org_id = $1",
            &[&org_id],
        )
        .await
        .unwrap()
        .get(0);
    let queued: i64 = client
        .query_one(
            "SELECT count(*) FROM public.jobs WHERE org_id = $1 AND kind = 'apply_corrections'",
            &[&org_id],
        )
        .await
        .unwrap()
        .get(0);
    (staged, queued)
}

#[tokio::test]
async fn only_approved_reviews_are_staged_and_only_while_the_tag_lives() {
    let Ok(dsn) = std::env::var("ER_TEST_PG_DSN") else {
        eprintln!("skipping Postgres approval tests (set ER_TEST_PG_DSN to run)");
        return;
    };
    let mut client = pg::connect(&dsn).await.unwrap();
    client
        .batch_execute(
            "DROP TABLE IF EXISTS taggings, tags, pair_labels, webhook_deliveries, \
             matching_models, staged_steward_actions, audit_log, webhooks, schedules, \
             config_versions, api_keys, jobs, orgs CASCADE;",
        )
        .await
        .unwrap();
    pg::ensure_schema(&mut client).await.unwrap();

    let app = app(AppState::with_pg("op-secret".to_string(), dsn.clone()));
    let dir = std::env::temp_dir().join(format!("er_approval_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let plain_path = dir.join("plain.yaml");
    let gated_path = dir.join("gated.yaml");
    std::fs::write(&plain_path, DEFAULT_YAML).unwrap();
    std::fs::write(
        &gated_path,
        format!("{DEFAULT_YAML}\napproval:\n  tag: approved-for-merge\n"),
    )
    .unwrap();

    // Starts WITHOUT a gate, so phase 1 measures the default.
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs",
            "op-secret",
            Body::from(
                serde_json::json!({
                    "name": "acme",
                    "config_path": plain_path.to_string_lossy(),
                    "env": { "ER_CONFIG_PATH": plain_path.to_string_lossy() },
                })
                .to_string(),
            ),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let org_id = pg::get_org(&client, "acme").await.unwrap().unwrap().id;

    let keys = |role: &str| {
        let app = app.clone();
        let role = role.to_string();
        async move {
            let resp = app
                .oneshot(req(
                    "POST",
                    "/v1/orgs/acme/api-keys",
                    "op-secret",
                    Body::from(serde_json::json!({ "role": role }).to_string()),
                ))
                .await
                .unwrap();
            json_body(resp).await["key"].as_str().unwrap().to_string()
        }
    };
    let admin_key = keys("admin").await;
    let steward_key = keys("steward").await;

    // A tag, applied to two reviews. The ids are opaque to this job: a staged
    // resolve carries only the review id, which is why the gate needs no lake.
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs/acme/tags",
            &steward_key,
            Body::from(serde_json::json!({ "name": "approved-for-merge" }).to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let tag_id = json_body(resp).await["id"].as_str().unwrap().to_string();

    for review in ["01jmrev0000000000000000001", "01jmrev0000000000000000002"] {
        let resp = app
            .clone()
            .oneshot(req(
                "POST",
                &format!("/v1/orgs/acme/tags/{tag_id}/apply"),
                &steward_key,
                Body::from(
                    serde_json::json!({ "subject_type": "review", "subject_id": review })
                        .to_string(),
                ),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }
    // Something tagged that is NOT a review, to prove the job's subject filter
    // is real: an entity carrying the approval tag must not be resolved as one.
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            &format!("/v1/orgs/acme/tags/{tag_id}/apply"),
            &steward_key,
            Body::from(
                serde_json::json!({ "subject_type": "entity", "subject_id": "01jm0e000000" })
                    .to_string(),
            ),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let run = || {
        let app = app.clone();
        let admin_key = admin_key.clone();
        async move {
            let resp = app
                .clone()
                .oneshot(req(
                    "POST",
                    "/v1/orgs/acme/jobs",
                    &admin_key,
                    Body::from(serde_json::json!({ "kind": "apply_approved" }).to_string()),
                ))
                .await
                .unwrap();
            assert_eq!(
                resp.status(),
                StatusCode::ACCEPTED,
                "the kind is schedulable"
            );
            er_server::dispatcher::dispatch_once_with(
                &pg::connect(&std::env::var("ER_TEST_PG_DSN").unwrap())
                    .await
                    .unwrap(),
                &Ok(None),
                &er_server::mail::Mailer::off(),
            )
            .await
            .unwrap()
        }
    };

    // ---- phase 1: no gate configured, so nothing is staged -------------------
    assert!(run().await, "the job should have been claimed");
    let (staged, queued) = staged_and_queued(&client, &org_id).await;
    assert_eq!(
        (staged, queued),
        (0, 0),
        "a tenant with no approval.tag must stage nothing: an approval workflow \
         that is on until switched off would merge groups nobody looked at"
    );

    // ---- phase 2: the gate configured, so the two reviews are staged ---------
    client
        .execute(
            "UPDATE public.orgs SET env = $2 WHERE id = $1",
            &[
                &org_id,
                &serde_json::json!({ "ER_CONFIG_PATH": gated_path.to_string_lossy() }),
            ],
        )
        .await
        .unwrap();

    assert!(run().await);
    let (staged, queued) = staged_and_queued(&client, &org_id).await;
    assert_eq!(staged, 2, "both approved REVIEWS staged, and only those");
    assert_eq!(queued, 1, "one apply enqueued to work through them");

    // The staged action is a resolve-as-match on the review, which is the
    // existing steward path rather than a new way to merge.
    let action: serde_json::Value = client
        .query_one(
            "SELECT action FROM public.staged_steward_actions WHERE org_id = $1 LIMIT 1",
            &[&org_id],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(action["type"], "resolve_review");
    assert_eq!(
        action["resolution"], "match",
        "approving a duplicate group means the records are the same; a steward who \
         means no_match resolves it directly"
    );

    // The tag came off as each was staged, so the entity tagging is all that is
    // left — and a second tick is a no-op rather than a re-stage.
    let remaining: i64 = client
        .query_one(
            "SELECT count(*) FROM public.taggings WHERE org_id = $1 AND subject_type = 'review'",
            &[&org_id],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(
        remaining, 0,
        "an approved review is untagged as it is staged"
    );

    assert!(run().await);
    let (staged_again, _) = staged_and_queued(&client, &org_id).await;
    assert_eq!(
        staged_again, 2,
        "a second tick with nothing newly approved stages nothing: the job is \
         idempotent because the tag is what carries the approval"
    );

    // ---- phase 3: retiring the tag disables the workflow ---------------------
    for review in ["01jmrev0000000000000000003"] {
        let resp = app
            .clone()
            .oneshot(req(
                "POST",
                &format!("/v1/orgs/acme/tags/{tag_id}/apply"),
                &steward_key,
                Body::from(
                    serde_json::json!({ "subject_type": "review", "subject_id": review })
                        .to_string(),
                ),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }
    let resp = app
        .clone()
        .oneshot(req(
            "DELETE",
            &format!("/v1/orgs/acme/tags/{tag_id}"),
            &admin_key,
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    assert!(run().await);
    let (after_retire, _) = staged_and_queued(&client, &org_id).await;
    assert_eq!(
        after_retire, 2,
        "retiring the tag disables the gate — the config names a tag by NAME and \
         the lookup is of live tags only, so a retired gate stages nothing even \
         though a review still carries the tagging"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
