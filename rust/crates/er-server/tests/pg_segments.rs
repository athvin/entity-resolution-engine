//! Segment scoping through the API (behind `ER_TEST_PG_DSN`).
//!
//! A segment bounds how much of the corpus one run re-scores (#54). The thing
//! worth testing at this layer is not the scoring — that is `lake_e2e`'s
//! `resolving_each_segment_in_turn_lands_where_one_full_run_would` — but the
//! contract around it:
//!
//! 1. A run carries a segment **name**. The predicate stays in the published
//!    config, which has been parsed and column-checked at publish. If a request
//!    could carry SQL, every tenant with steward rights would have an injection
//!    point into the engine's own queries.
//! 2. An unknown name is refused **at submit**, naming what does exist. The
//!    alternative is a job that queues, gets claimed, and then fails — which
//!    looks like an outage rather than a typo.
//! 3. The names are discoverable, because a caller has to know them to use them.
//!
//! Run with e.g.
//!   ER_TEST_PG_DSN='host=localhost port=5433 user=er password=er dbname=er' \
//!     cargo test -p er-server --test pg_segments

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
async fn a_run_names_a_segment_and_an_unknown_name_is_refused_at_submit() {
    let Ok(dsn) = std::env::var("ER_TEST_PG_DSN") else {
        eprintln!("skipping Postgres segment tests (set ER_TEST_PG_DSN to run)");
        return;
    };
    let mut client = pg::connect(&dsn).await.unwrap();
    client
        .batch_execute(
            "DROP TABLE IF EXISTS webhook_deliveries, webhooks, staged_steward_actions, audit_log, schedules, config_versions, api_keys, jobs, orgs CASCADE;",
        )
        .await
        .unwrap();
    pg::ensure_schema(&mut client).await.unwrap();

    let app = app(AppState::with_pg("op-secret".to_string(), dsn.clone()));
    let config_path = std::env::temp_dir().join(format!("er_seg_{}.yaml", std::process::id()));
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
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs/acme/api-keys",
            "op-secret",
            Body::from(r#"{"role":"admin"}"#),
        ))
        .await
        .unwrap();
    let admin_key = json_body(resp).await["key"].as_str().unwrap().to_string();
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs/acme/api-keys",
            "op-secret",
            Body::from(r#"{"role":"viewer"}"#),
        ))
        .await
        .unwrap();
    let viewer_key = json_body(resp).await["key"].as_str().unwrap().to_string();

    // With no config published there are no segments to name, and the catalog
    // says so the same way every other config-derived read does — rather than
    // answering with an empty list, which would read as "none declared".
    let resp = app
        .clone()
        .oneshot(req(
            "GET",
            "/v1/orgs/acme/segments",
            &viewer_key,
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::UNPROCESSABLE_ENTITY,
        "no config published yet"
    );

    let scoped = format!(
        "{DEFAULT_YAML}\nsegments:\n  \
         west: \"addr_region in ('CA','OR','WA')\"\n  \
         from_crm: \"source_system = 'crm'\"\n"
    );
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs/acme/config/versions",
            &admin_key,
            Body::from(serde_json::json!({ "yaml": scoped }).to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let version = json_body(resp).await["version"].as_i64().unwrap();
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            &format!("/v1/orgs/acme/config/versions/{version}/publish"),
            &admin_key,
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "a config with valid segments publishes: {:?}",
        json_body(resp).await
    );

    // Discovery, with the predicate: an operator splitting a long run needs to
    // see what each slice covers, and it is their own published config.
    let resp = app
        .clone()
        .oneshot(req(
            "GET",
            "/v1/orgs/acme/segments",
            &viewer_key,
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let listed = json_body(resp).await;
    let names: Vec<&str> = listed["segments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["from_crm", "west"]);
    assert_eq!(
        listed["segments"][1]["predicate"].as_str(),
        Some("addr_region in ('CA','OR','WA')")
    );

    // A declared segment is accepted, and the NAME is what lands in the job —
    // the dispatcher resolves it against the config it is about to run under.
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs/acme/jobs",
            &admin_key,
            Body::from(r#"{"kind":"run_all_incremental","segment":"west"}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let job = json_body(resp).await;
    assert_eq!(job["params"]["segment"].as_str(), Some("west"));
    assert!(
        !job["params"].to_string().contains("addr_region"),
        "the job must carry the name, not the predicate: {}",
        job["params"]
    );

    // An unknown name is refused here, and the refusal names what exists. A
    // typo must not become a queued job that fails minutes later.
    for bad in ["easr", "WEST", ""] {
        let resp = app
            .clone()
            .oneshot(req(
                "POST",
                "/v1/orgs/acme/jobs",
                &admin_key,
                Body::from(
                    serde_json::json!({ "kind": "run_all_incremental", "segment": bad })
                        .to_string(),
                ),
            ))
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "segment {bad:?} should be refused"
        );
        let detail = json_body(resp).await["detail"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(
            detail.contains("west") && detail.contains("from_crm"),
            "the refusal must name the declared segments: {detail}"
        );
    }

    // Omitting the segment is the ordinary whole-corpus run, with no params.
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs/acme/jobs",
            &admin_key,
            Body::from(r#"{"kind":"run_all_incremental"}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let job = json_body(resp).await;
    assert!(
        job["params"]["segment"].is_null(),
        "an unscoped run must carry no segment: {}",
        job["params"]
    );

    // A viewer cannot start a run at all, scoped or not.
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs/acme/jobs",
            &viewer_key,
            Body::from(r#"{"kind":"run_all_incremental","segment":"west"}"#),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    let _ = std::fs::remove_file(&config_path);
}

#[tokio::test]
async fn a_segment_whose_sql_is_broken_cannot_be_published() {
    let Ok(dsn) = std::env::var("ER_TEST_PG_DSN") else {
        eprintln!("skipping Postgres segment publish test (set ER_TEST_PG_DSN to run)");
        return;
    };
    let mut client = pg::connect(&dsn).await.unwrap();
    client
        .batch_execute(
            "DROP TABLE IF EXISTS webhook_deliveries, webhooks, staged_steward_actions, audit_log, schedules, config_versions, api_keys, jobs, orgs CASCADE;",
        )
        .await
        .unwrap();
    pg::ensure_schema(&mut client).await.unwrap();

    let app = app(AppState::with_pg("op-secret".to_string(), dsn.clone()));
    let config_path = std::env::temp_dir().join(format!("er_segbad_{}.yaml", std::process::id()));
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
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs/acme/api-keys",
            "op-secret",
            Body::from(r#"{"role":"admin"}"#),
        ))
        .await
        .unwrap();
    let admin_key = json_body(resp).await["key"].as_str().unwrap().to_string();

    // Publish is the only gate a segment predicate ever passes through: a run
    // names a segment, so nothing downstream re-parses the SQL. A typo that
    // goes live becomes a failing run for every tenant that triggers it.
    //
    // A draft version is deliberately storable — drafts are work in progress,
    // and the column analysis runs on the way to ACTIVE (`publish_config`), not
    // on the way to the table.
    let bad = format!("{DEFAULT_YAML}\nsegments:\n  west: \"addr_regoin = 'CA'\"\n");
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs/acme/config/versions",
            &admin_key,
            Body::from(serde_json::json!({ "yaml": bad }).to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::CREATED,
        "a draft is storable; publish is the gate"
    );
    let version = json_body(resp).await["version"].as_i64().unwrap();

    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            &format!("/v1/orgs/acme/config/versions/{version}/publish"),
            &admin_key,
            Body::empty(),
        ))
        .await
        .unwrap();
    let status = resp.status();
    let detail = json_body(resp).await.to_string();
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "a segment on an unknown column must not go live: {detail}"
    );
    assert!(
        detail.contains("addr_regoin") && detail.contains("segment `west`"),
        "the refusal must point at the offending segment and column: {detail}"
    );

    // And nothing went live, so a run cannot pick up the bad predicate.
    let resp = app
        .clone()
        .oneshot(req(
            "GET",
            "/v1/orgs/acme/segments",
            &admin_key,
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::UNPROCESSABLE_ENTITY,
        "the refused version must not have become the active config"
    );

    let _ = std::fs::remove_file(&config_path);
}
