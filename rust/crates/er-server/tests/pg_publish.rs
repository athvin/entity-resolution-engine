//! Config publish over the real Postgres backend (behind `ER_TEST_PG_DSN`):
//! first-publish tier C (train + rebuild jobs), an A-tier threshold change, the
//! operator-only-block refusal (403 for admin, allowed for operator), the file
//! written to the org's config_path, active_config_version repointing, and the
//! missing-version 404 — all through the HTTP surface. Run with e.g.
//!   ER_TEST_PG_DSN='host=localhost port=5433 user=er password=er dbname=er' \
//!     cargo test -p er-server --test pg_publish

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

fn create_body(yaml: &str) -> Body {
    Body::from(serde_json::json!({ "yaml": yaml }).to_string())
}

async fn json_body(resp: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

/// Create a draft version (admin) and return its number.
async fn create_version(app: &axum::Router, bearer: &str, yaml: &str) -> i64 {
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs/acme/config/versions",
            bearer,
            create_body(yaml),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    json_body(resp).await["version"].as_i64().unwrap()
}

async fn publish(app: &axum::Router, bearer: &str, version: i64) -> axum::response::Response {
    app.clone()
        .oneshot(req(
            "POST",
            &format!("/v1/orgs/acme/config/versions/{version}/publish"),
            bearer,
            Body::empty(),
        ))
        .await
        .unwrap()
}

#[tokio::test]
async fn config_publish_tiers_and_authz() {
    let Ok(dsn) = std::env::var("ER_TEST_PG_DSN") else {
        eprintln!("skipping Postgres publish e2e (set ER_TEST_PG_DSN to run)");
        return;
    };

    let mut client = pg::connect(&dsn).await.unwrap();
    client
        .batch_execute(
            "DROP TABLE IF EXISTS staged_steward_actions, audit_log, schedules, config_versions, api_keys, jobs, orgs CASCADE;",
        )
        .await
        .unwrap();
    pg::ensure_schema(&mut client).await.unwrap();

    let app = app(AppState::with_pg("op-secret".to_string(), dsn.clone()));

    // An org whose config_path points at a writable temp file.
    let config_path = std::env::temp_dir().join(format!("er_pub_{}.yaml", std::process::id()));
    let config_path = config_path.to_string_lossy().to_string();
    let _ = std::fs::remove_file(&config_path);
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs",
            "op-secret",
            Body::from(
                // `ER_MODEL_PATH` so creating the org SEEDS and activates its
                // starter model, as a real tenant's does — the ensemble phases
                // below need a registry name that actually resolves, and a
                // tenant with no model at all could never publish a `models:`
                // list anyway.
                serde_json::json!({
                    "name": "acme",
                    "config_path": config_path,
                    "env": {
                        "ER_MODEL_PATH": std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                            .join("../../../fixtures/static/model_test_v1.json")
                            .to_string_lossy(),
                        "ER_MODEL_VERSION": "v0001",
                        "ER_TF_SNAPSHOT_ID": "01JWMDTFSNAP00000000000001",
                    },
                })
                .to_string(),
            ),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    // An admin key: admins create + publish non-structural changes.
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

    // v1 == default. First publish by the admin -> tier C, train + rebuild jobs.
    let v1 = create_version(&app, &admin_key, DEFAULT_YAML).await;
    assert_eq!(v1, 1);
    let resp = publish(&app, &admin_key, v1).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let pub1 = json_body(resp).await;
    assert_eq!(pub1["tier"], "C");
    assert_eq!(pub1["jobs_enqueued"].as_array().unwrap().len(), 2);
    assert!(pub1["published_by"].as_str().unwrap().starts_with("key:"));

    // The YAML was written to config_path.
    let written = std::fs::read_to_string(&config_path).unwrap();
    assert!(written.contains("tenant: default"));

    // active config now resolves to v1, published.
    let resp = app
        .clone()
        .oneshot(req(
            "GET",
            "/v1/orgs/acme/config",
            &admin_key,
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let active = json_body(resp).await;
    assert_eq!(active["version"], 1);
    assert_eq!(active["state"], "published");

    // Publishing turns `correction_pass.cadence` into a real schedule. The
    // cadence had been stored by every tenant config and read by nothing, so the
    // periodic correction pass it describes never ran.
    let read_schedules = async |bearer: &str| -> Vec<serde_json::Value> {
        let resp = app
            .clone()
            .oneshot(req("GET", "/v1/orgs/acme/schedules", bearer, Body::empty()))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        json_body(resp).await.as_array().cloned().unwrap()
    };
    let rows = read_schedules(&admin_key).await;
    let owned: Vec<&serde_json::Value> = rows
        .iter()
        .filter(|r| r["source"] == "config:correction_pass")
        .collect();
    assert_eq!(
        owned.len(),
        1,
        "publishing must own exactly one correction-pass schedule"
    );
    assert_eq!(owned[0]["kind"], "correct");
    assert_eq!(
        owned[0]["cron"], "0 3 * * 0",
        "the schedule must carry the published cadence"
    );

    // v2 changes only thresholds (an A-tier block). Publish -> tier A, 1 job.
    let v2_yaml = DEFAULT_YAML.replace("review_low: 0.60", "review_low: 0.55");
    assert_ne!(v2_yaml, DEFAULT_YAML, "the threshold edit must apply");
    let v2 = create_version(&app, &admin_key, &v2_yaml).await;
    let resp = publish(&app, &admin_key, v2).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let pub2 = json_body(resp).await;
    assert_eq!(pub2["tier"], "A");
    let changed = pub2["changed_blocks"].as_array().unwrap();
    assert!(changed.iter().any(|b| b == "thresholds"));
    assert_eq!(pub2["jobs_enqueued"].as_array().unwrap().len(), 1);

    // v3 changes generator (an operator-only block) ON TOP of the active config
    // (v2), so the diff versus active is *only* generator -> tier None. (Publish
    // classifies against the current active config, not the original.)
    let v3_yaml = v2_yaml.replace("seed: 42", "seed: 7");
    assert_ne!(v3_yaml, v2_yaml, "the generator edit must apply");
    let v3 = create_version(&app, &admin_key, &v3_yaml).await;

    // The admin cannot publish an operator-only change -> 403, with the FastAPI
    // `{detail}` error body the BFF parses (not a bare `{error}`).
    let resp = publish(&app, &admin_key, v3).await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let body = json_body(resp).await;
    assert!(
        body["detail"]
            .as_str()
            .is_some_and(|d| d.contains("operator-only")),
        "403 body carries a {{detail}} message: {body}"
    );

    // The operator can, and it is tier None (generator is not an A/B/C block).
    let resp = publish(&app, "op-secret", v3).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let pub3 = json_body(resp).await;
    assert!(pub3["tier"].is_null());
    assert!(
        pub3["changed_blocks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|b| b == "generator")
    );
    assert_eq!(pub3["jobs_enqueued"].as_array().unwrap().len(), 0);
    assert_eq!(pub3["published_by"], "operator");

    // active config now resolves to v3.
    let resp = app
        .clone()
        .oneshot(req(
            "GET",
            "/v1/orgs/acme/config",
            "op-secret",
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(json_body(resp).await["version"], 3);

    // Publishing a non-existent version is a 404.
    let resp = publish(&app, "op-secret", 99).await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    // Three publishes later there is still exactly one correction-pass schedule:
    // the owner is reconciled, not re-created.
    let rows = read_schedules("op-secret").await;
    assert_eq!(
        rows.iter()
            .filter(|r| r["source"] == "config:correction_pass")
            .count(),
        1,
        "republishing must not accumulate correction-pass schedules"
    );

    // Changing the cadence re-points that same schedule.
    let v4_yaml = DEFAULT_YAML.replace("cadence: \"0 3 * * 0\"", "cadence: \"30 4 * * 1\"");
    assert_ne!(v4_yaml, DEFAULT_YAML, "the cadence edit must apply");
    let v4 = create_version(&app, "op-secret", &v4_yaml).await;
    let resp = publish(&app, "op-secret", v4).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let rows = read_schedules("op-secret").await;
    let owned: Vec<&serde_json::Value> = rows
        .iter()
        .filter(|r| r["source"] == "config:correction_pass")
        .collect();
    assert_eq!(owned.len(), 1, "still one schedule after a cadence change");
    assert_eq!(
        owned[0]["cron"], "30 4 * * 1",
        "the schedule must follow the newly published cadence"
    );

    // ─── E5b: publishing an ensemble ────────────────────────────────────────
    //
    // A `models:` member must resolve to an ACTIVE registry version. Refused at
    // PUBLISH, as a 422 naming the member, rather than at run time: a publish
    // has a human waiting on the response, while the same mistake found in the
    // dispatcher is a failed job with the tenant's rebuild queued behind it.
    let bad_yaml = format!("{v4_yaml}\nmodels:\n  - name: nonesuch\n");
    let bad = create_version(&app, "op-secret", &bad_yaml).await;
    let resp = publish(&app, "op-secret", bad).await;
    assert_eq!(
        resp.status(),
        StatusCode::UNPROCESSABLE_ENTITY,
        "a member with no active version must not publish"
    );
    let body = json_body(resp).await;
    let detail = body["detail"].as_str().unwrap_or_default();
    assert!(
        detail.contains("nonesuch") && detail.contains("no ACTIVE version"),
        "the 422 must name the member: {body}"
    );

    // `default` IS active — creating the org seeded and activated it — so an
    // ensemble naming it publishes, and lands as tier **B**: changing which
    // models score needs a re-score, not a retrain, so one rebuild job and no
    // train job.
    let ens_yaml = format!("{v4_yaml}\nmodels:\n  - name: default\n");
    let ens = create_version(&app, "op-secret", &ens_yaml).await;
    let resp = publish(&app, "op-secret", ens).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let pub_ens = json_body(resp).await;
    assert_eq!(
        pub_ens["tier"], "B",
        "a models: change is a re-score, not a retrain: {pub_ens}"
    );
    assert!(
        pub_ens["changed_blocks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|b| b == "models"),
        "the changed set must name the block: {pub_ens}"
    );
    assert_eq!(
        pub_ens["jobs_enqueued"].as_array().unwrap().len(),
        1,
        "tier B enqueues the rebuild only, no train"
    );

    let _ = std::fs::remove_file(&config_path);
}
