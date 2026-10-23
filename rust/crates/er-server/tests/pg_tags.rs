//! Tags: the vocabulary a steward sorts work by (behind `ER_TEST_PG_DSN`).
//!
//! What these assertions pin down:
//!   * the role ladder — a viewer reads, a steward names and applies, an admin
//!     retires. Retiring is admin because it affects every subject the tag was
//!     applied to and every filter that names it;
//!   * applying twice is ONE row, enforced by the unique index rather than by a
//!     read-then-write two stewards clicking at once would race;
//!   * a closed subject-kind set, because an unconstrained value is accepted
//!     and then matches nothing on read — the silence `webhook.events` was
//!     closed to prevent;
//!   * **retiring a tag that has been APPLIED works.** This is the regression
//!     test for the class D11 found in webhooks, where the delete worked right
//!     up until the subscription had delivered something and then failed on a
//!     NO ACTION foreign key for exactly the rows that had been used. A tag is
//!     soft-retired for that reason, and the only way to know is to apply one
//!     first.
//!
//! Run with e.g.
//!   ER_TEST_PG_DSN='host=localhost port=5466 user=er password=er dbname=er' \
//!     cargo test -p er-server --test pg_tags

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

#[tokio::test]
async fn tags_are_named_applied_and_retired_without_losing_what_they_touched() {
    let Ok(dsn) = std::env::var("ER_TEST_PG_DSN") else {
        eprintln!("skipping Postgres tag tests (set ER_TEST_PG_DSN to run)");
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
    let config_path = std::env::temp_dir().join(format!("er_tags_{}.yaml", std::process::id()));
    std::fs::write(&config_path, DEFAULT_YAML).unwrap();
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs",
            "op-secret",
            Body::from(
                serde_json::json!({
                    "name": "acme",
                    "config_path": config_path.to_string_lossy(),
                })
                .to_string(),
            ),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
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
    let viewer_key = keys("viewer").await;

    // ---- phase 1: the role ladder on naming ---------------------------------
    let make = |key: &str, name: &str| {
        let app = app.clone();
        let key = key.to_string();
        let name = name.to_string();
        async move {
            app.oneshot(req(
                "POST",
                "/v1/orgs/acme/tags",
                &key,
                Body::from(serde_json::json!({ "name": name }).to_string()),
            ))
            .await
            .unwrap()
        }
    };

    // A viewer cannot name work.
    assert_eq!(
        make(&viewer_key, "needs-a-human").await.status(),
        StatusCode::FORBIDDEN
    );
    // A steward can: naming the work is part of doing it, and a steward who
    // cannot would put the distinction in a note nothing can filter on.
    let resp = make(&steward_key, "needs-a-human").await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let tag = json_body(resp).await;
    let tag_id = tag["id"].as_str().unwrap().to_string();
    assert_eq!(tag["name"], "needs-a-human");
    assert_eq!(tag["active"], true);
    assert!(
        tag["created_by"].as_str().is_some_and(|a| !a.is_empty()),
        "the tag records who created it, because tagging is how work is assigned: {tag}"
    );

    // The same name twice is a 409 from the partial unique index, not two tags
    // one filter cannot tell apart.
    assert_eq!(
        make(&steward_key, "needs-a-human").await.status(),
        StatusCode::CONFLICT
    );
    // Whitespace is trimmed, so " needs-a-human " is the same name.
    assert_eq!(
        make(&steward_key, "  needs-a-human  ").await.status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        make(&steward_key, "   ").await.status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );

    // ---- phase 2: applying ---------------------------------------------------
    let apply = |verb: &str, key: &str, kind: &str, id: &str| {
        let app = app.clone();
        let (verb, key, kind, id) = (
            verb.to_string(),
            key.to_string(),
            kind.to_string(),
            id.to_string(),
        );
        let tag_id = tag_id.clone();
        async move {
            app.oneshot(req(
                "POST",
                &format!("/v1/orgs/acme/tags/{tag_id}/{verb}"),
                &key,
                Body::from(
                    serde_json::json!({ "subject_type": kind, "subject_id": id }).to_string(),
                ),
            ))
            .await
            .unwrap()
        }
    };

    assert_eq!(
        apply("apply", &viewer_key, "entity", "E1").await.status(),
        StatusCode::FORBIDDEN,
        "application is role-restricted"
    );
    assert_eq!(
        apply("apply", &steward_key, "entity", "E1").await.status(),
        StatusCode::NO_CONTENT
    );
    // Twice is the same fact. The unique index makes the second a no-op, so two
    // stewards clicking at once cannot produce two rows.
    assert_eq!(
        apply("apply", &steward_key, "entity", "E1").await.status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        apply("apply", &steward_key, "review", "R7").await.status(),
        StatusCode::NO_CONTENT
    );
    // A kind outside the closed set is refused, rather than written and then
    // matching nothing any filter looks for.
    assert_eq!(
        apply("apply", &steward_key, "entities", "E1")
            .await
            .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );

    // ---- phase 3: reading ----------------------------------------------------
    let get = |uri: &str, key: &str| {
        let app = app.clone();
        let (uri, key) = (uri.to_string(), key.to_string());
        async move {
            app.oneshot(req("GET", &uri, &key, Body::empty()))
                .await
                .unwrap()
        }
    };

    let listed = json_body(get("/v1/orgs/acme/tags", &viewer_key).await).await;
    assert_eq!(
        listed["tags"].as_array().map(Vec::len),
        Some(1),
        "one tag, not one per failed attempt: {listed}"
    );

    // The chips on one subject.
    let chips = json_body(
        get(
            "/v1/orgs/acme/tags?subject_type=entity&subject_id=E1",
            &viewer_key,
        )
        .await,
    )
    .await;
    assert_eq!(chips["tags"][0]["name"], "needs-a-human");
    let none = json_body(
        get(
            "/v1/orgs/acme/tags?subject_type=entity&subject_id=E-never-tagged",
            &viewer_key,
        )
        .await,
    )
    .await;
    assert_eq!(none["tags"].as_array().map(Vec::len), Some(0));

    // Half a subject pair is a caller bug, not a request for the whole
    // vocabulary: answering the unfiltered list would read as "this thing has
    // every tag".
    assert_eq!(
        get("/v1/orgs/acme/tags?subject_type=entity", &viewer_key)
            .await
            .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );

    // What carries the tag — the scope a bulk action by tag will read.
    let subjects = json_body(
        get(
            &format!("/v1/orgs/acme/tags/{tag_id}/subjects"),
            &viewer_key,
        )
        .await,
    )
    .await;
    assert_eq!(
        subjects["subjects"].as_array().map(Vec::len),
        Some(2),
        "the entity and the review, and the double apply is still one row: {subjects}"
    );
    let only_reviews = json_body(
        get(
            &format!("/v1/orgs/acme/tags/{tag_id}/subjects?subject_type=review"),
            &viewer_key,
        )
        .await,
    )
    .await;
    assert_eq!(only_reviews["subjects"].as_array().map(Vec::len), Some(1));

    // ---- phase 4: removing one application -----------------------------------
    assert_eq!(
        apply("unapply", &steward_key, "review", "R7")
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        apply("unapply", &steward_key, "review", "R7")
            .await
            .status(),
        StatusCode::NOT_FOUND,
        "removing what is not applied is a 404 on the thing the caller named"
    );

    // ---- phase 5: retiring a tag that HAS been applied -----------------------
    // The D11 regression. `taggings.tag_id` references `tags.id` with NO ACTION,
    // so a hard DELETE would fail here and ONLY here — for a tag that had
    // actually been used, which is every tag that ever mattered. The entity is
    // soft-retired for exactly this reason, and applying one first is the only
    // way the test can tell.
    assert_eq!(
        app.clone()
            .oneshot(req(
                "DELETE",
                &format!("/v1/orgs/acme/tags/{tag_id}"),
                &steward_key,
                Body::empty(),
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN,
        "retiring is admin: it affects every subject the tag touched"
    );
    assert_eq!(
        app.clone()
            .oneshot(req(
                "DELETE",
                &format!("/v1/orgs/acme/tags/{tag_id}"),
                &admin_key,
                Body::empty(),
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::NO_CONTENT,
        "a tag with applications must still be retirable — the whole point of the soft delete"
    );

    // Retired is indistinguishable from gone to every reader.
    let after = json_body(get("/v1/orgs/acme/tags", &viewer_key).await).await;
    assert_eq!(after["tags"].as_array().map(Vec::len), Some(0));
    let chips_after = json_body(
        get(
            "/v1/orgs/acme/tags?subject_type=entity&subject_id=E1",
            &viewer_key,
        )
        .await,
    )
    .await;
    assert_eq!(
        chips_after["tags"].as_array().map(Vec::len),
        Some(0),
        "a retired tag stops appearing as a chip without the tagging being rewritten"
    );
    // And applying a retired tag is a 404 rather than resurrecting it.
    assert_eq!(
        apply("apply", &steward_key, "entity", "E2").await.status(),
        StatusCode::NOT_FOUND
    );
    // The name is free again, which is what the PARTIAL index buys.
    assert_eq!(
        make(&steward_key, "needs-a-human").await.status(),
        StatusCode::CREATED
    );

    // The tagging row survived the retirement: what a group was marked as is
    // part of why it was handled that way.
    let surviving: i64 = client
        .query_one("SELECT count(*) FROM public.taggings", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(
        surviving, 1,
        "retiring a tag must not delete what it was applied to"
    );

    let _ = std::fs::remove_file(&config_path);
}
