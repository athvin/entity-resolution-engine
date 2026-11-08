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

use crate::common;

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
    let Some(db) = common::fresh_db("tags").await else {
        return;
    };
    let dsn = db.dsn().to_string();
    let mut client = pg::connect(&dsn).await.unwrap();
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
                &format!("/v1/orgs/acme/datasets/default/tags/{tag_id}/{verb}"),
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
        StatusCode::OK
    );
    // Twice is the same fact. The unique index makes the second a no-op, so two
    // stewards clicking at once cannot produce two rows.
    assert_eq!(
        apply("apply", &steward_key, "entity", "E1").await.status(),
        StatusCode::OK
    );
    assert_eq!(
        apply("apply", &steward_key, "review", "R7").await.status(),
        StatusCode::OK
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
            &format!("/v1/orgs/acme/datasets/default/tags/{tag_id}/subjects"),
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
            &format!("/v1/orgs/acme/datasets/default/tags/{tag_id}/subjects?subject_type=review"),
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
        StatusCode::OK
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
        StatusCode::OK,
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

/// The spec's reserved system tags and per-tag permission (#805):
///   * the five reserved names cannot be user-created (422, case-insensitive —
///     the unique index is case-sensitive, so `Merge` would otherwise slip
///     past), and a system tag cannot be retired or hand-applied;
///   * `apply_min_role` is enforced as a 403 on role — never a 404 — per the
///     house rule, on unapply as well as apply;
///   * the bulk taggings read the export prequeries.
#[tokio::test]
async fn system_tags_are_reserved_and_apply_min_role_is_enforced() {
    let Some(db) = common::fresh_db("tags_sys").await else {
        return;
    };
    let dsn = db.dsn().to_string();
    let mut client = pg::connect(&dsn).await.unwrap();
    pg::ensure_schema(&mut client).await.unwrap();

    let app = app(AppState::with_pg("op-secret".to_string(), dsn.clone()));
    let config_path = std::env::temp_dir().join(format!("er_tags_sys_{}.yaml", std::process::id()));
    std::fs::write(&config_path, DEFAULT_YAML).unwrap();
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs",
            "op-secret",
            Body::from(
                serde_json::json!({
                    "name": "globex",
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
                    "/v1/orgs/globex/api-keys",
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

    let make = |key: &str, body: serde_json::Value| {
        let app = app.clone();
        let key = key.to_string();
        async move {
            app.oneshot(req(
                "POST",
                "/v1/orgs/globex/tags",
                &key,
                Body::from(body.to_string()),
            ))
            .await
            .unwrap()
        }
    };

    // ---- reserved names ------------------------------------------------------
    for reserved in ["merge", "manual_merge", "unmatch", "link", "split"] {
        assert_eq!(
            make(&steward_key, serde_json::json!({ "name": reserved }))
                .await
                .status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "{reserved} is the platform's to write, not a steward's to squat"
        );
    }
    // Case-insensitive: the unique index is case-sensitive, so a
    // case-sensitive check would let this through to collide later.
    assert_eq!(
        make(&steward_key, serde_json::json!({ "name": "Merge" }))
            .await
            .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    // Containing a reserved word is fine — only the exact name is the system's.
    assert_eq!(
        make(&steward_key, serde_json::json!({ "name": "merge-review" }))
            .await
            .status(),
        StatusCode::CREATED
    );

    // ---- apply_min_role ------------------------------------------------------
    assert_eq!(
        make(
            &steward_key,
            serde_json::json!({ "name": "bad-floor", "apply_min_role": "viewer" }),
        )
        .await
        .status(),
        StatusCode::UNPROCESSABLE_ENTITY,
        "viewer is below the route's floor"
    );
    let resp = make(
        &steward_key,
        serde_json::json!({ "name": "exec-decision", "apply_min_role": "admin" }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let admin_tag = json_body(resp).await;
    let admin_tag_id = admin_tag["id"].as_str().unwrap().to_string();
    assert_eq!(admin_tag["apply_min_role"], "admin");
    assert_eq!(admin_tag["kind"], "user");

    let apply_tag = |verb: &str, key: &str, tag_id: &str| {
        let app = app.clone();
        let (verb, key, tag_id) = (verb.to_string(), key.to_string(), tag_id.to_string());
        async move {
            app.oneshot(req(
                "POST",
                &format!("/v1/orgs/globex/datasets/default/tags/{tag_id}/{verb}"),
                &key,
                Body::from(
                    serde_json::json!({ "subject_type": "entity", "subject_id": "E1" }).to_string(),
                ),
            ))
            .await
            .unwrap()
        }
    };

    // The house rule: same-org, insufficient role is a 403 — never a 404.
    let denied = apply_tag("apply", &steward_key, &admin_tag_id).await;
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    assert!(
        json_body(denied).await["detail"]
            .as_str()
            .is_some_and(|d| d.contains("requires admin role")),
        "the denial names the role the tag demands"
    );
    assert_eq!(
        apply_tag("unapply", &steward_key, &admin_tag_id)
            .await
            .status(),
        StatusCode::FORBIDDEN,
        "removing is the same guarded action"
    );
    assert_eq!(
        apply_tag("apply", &admin_key, &admin_tag_id).await.status(),
        StatusCode::OK
    );

    // ---- system tags ----------------------------------------------------------
    let org_id = pg::get_org(&client, "globex").await.unwrap().unwrap().id;
    let sys = pg::create_system_tag(&client, &org_id, "merge", None, "test")
        .await
        .unwrap()
        .expect("no live tag named merge exists");
    let sys_id = sys.id.clone();
    assert_eq!(sys.kind, "system");

    // The steward never reaches the system-tag check — retiring is admin-gated
    // at the route. The admin reaches it and is refused: a system tag is not
    // any role's to retire.
    assert_eq!(
        app.clone()
            .oneshot(req(
                "DELETE",
                &format!("/v1/orgs/globex/tags/{sys_id}"),
                &steward_key,
                Body::empty(),
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        app.clone()
            .oneshot(req(
                "DELETE",
                &format!("/v1/orgs/globex/tags/{sys_id}"),
                &admin_key,
                Body::empty(),
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::UNPROCESSABLE_ENTITY,
        "a system tag is not any role's to retire"
    );
    // Nor applies it by hand — a steward-marked `merge` would read identically
    // to the platform's own mark. The operator credential (what the platform's
    // writers authenticate as) is the exemption.
    assert_eq!(
        apply_tag("apply", &steward_key, &sys_id).await.status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        apply_tag("apply", &admin_key, &sys_id).await.status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        apply_tag("apply", "op-secret", &sys_id).await.status(),
        StatusCode::OK
    );

    // ---- the bulk taggings read the export prequeries -------------------------
    let get = |uri: &str, key: &str| {
        let app = app.clone();
        let (uri, key) = (uri.to_string(), key.to_string());
        async move {
            app.oneshot(req("GET", &uri, &key, Body::empty()))
                .await
                .unwrap()
        }
    };
    assert_eq!(
        get("/v1/orgs/globex/datasets/default/taggings", &viewer_key)
            .await
            .status(),
        StatusCode::UNPROCESSABLE_ENTITY,
        "subject_type is required — a default would read as 'everything'"
    );
    let taggings = json_body(
        get(
            "/v1/orgs/globex/datasets/default/taggings?subject_type=entity",
            &viewer_key,
        )
        .await,
    )
    .await;
    assert_eq!(taggings["truncated"], false);
    assert_eq!(
        taggings["taggings"].as_array().map(Vec::len),
        Some(2),
        "the admin's application and the operator's system-tag application: {taggings}"
    );

    let _ = std::fs::remove_file(&config_path);
}
