//! Labels, and the labelling queue that decides what to ask a human about
//! (behind `ER_TEST_PG_DSN`).
//!
//! The engine has always been able to learn from labels — `m_from_labels`,
//! `truth_space_from_labels_column`, `prediction_errors_from_labels_table` all
//! live in `dedupe-core::accuracy` — and there was no way to produce any. That
//! whole module had no caller outside `dedupe-cli` and its own golden tests, so
//! the product shipped a trainable engine with no training signal and no
//! accuracy number.
//!
//! What these assertions pin down:
//!   * a verdict is recorded, canonicalised to the order `match_scores` stores
//!     the pair in, and re-labelling CORRECTS rather than stacks;
//!   * nonsense is refused: an unknown verdict, a self-pair, an empty key;
//!   * coverage is reported split by verdict and origin, because a set that is
//!     95% `no_match` teaches a model almost nothing and a single total hides
//!     it;
//!   * role boundaries — a viewer reads, a steward writes.
//!
//! The uncertainty ORDERING is asserted in the DuckLake tier, where there are
//! real scores to order; here the queue is exercised for its contract and its
//! coverage summary.
//!
//! Run with e.g.
//!   ER_TEST_PG_DSN='host=localhost port=5433 user=er password=er dbname=er' \
//!     cargo test -p er-server --test pg_labels

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use er_server::{AppState, app, pg};
use tower::ServiceExt;

const DEFAULT_YAML: &str = include_str!("../../er-config/tests/fixtures/default.yaml");

/// The committed engine model, reused rather than hand-written: a document
/// `create_model_version` refuses is indistinguishable here from a bug in the
/// evaluation path.
const MODEL_JSON: &str = include_str!("../../../../fixtures/static/model_test_v1.json");

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
async fn labels_are_recorded_canonically_and_corrections_replace() {
    let Ok(dsn) = std::env::var("ER_TEST_PG_DSN") else {
        eprintln!("skipping Postgres label tests (set ER_TEST_PG_DSN to run)");
        return;
    };
    let mut client = pg::connect(&dsn).await.unwrap();
    client
        .batch_execute(
            "DROP TABLE IF EXISTS pair_labels, webhook_deliveries, matching_models, \
             staged_steward_actions, audit_log, webhooks, schedules, config_versions, \
             api_keys, jobs, orgs CASCADE;",
        )
        .await
        .unwrap();
    pg::ensure_schema(&mut client).await.unwrap();

    let app = app(AppState::with_pg("op-secret".to_string(), dsn.clone()));
    let config_path = std::env::temp_dir().join(format!("er_labels_{}.yaml", std::process::id()));
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
    let steward_key = keys("steward").await;
    let viewer_key = keys("viewer").await;

    // Recorded with the keys the WRONG way round on purpose: match_scores
    // stores a pair lower-key-first, so a label kept as given would describe a
    // pair nothing ever scored and would never match a score again.
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs/acme/labels",
            &steward_key,
            Body::from(
                serde_json::json!({
                    "rec_a_key": "crm:C009",
                    "rec_b_key": "billing:B001",
                    "verdict": "match",
                    "score_at_label": 0.83,
                    "model_version": "v0001",
                    "note": "same person, nickname",
                })
                .to_string(),
            ),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let label = json_body(resp).await;
    assert_eq!(
        (label["rec_a_key"].as_str(), label["rec_b_key"].as_str()),
        (Some("billing:B001"), Some("crm:C009")),
        "the pair must be stored in the canonical order match_scores uses"
    );
    assert_eq!(label["verdict"], "match");
    assert_eq!(label["origin"], "human");
    assert_eq!(label["score_at_label"], 0.83);

    // Re-labelling the SAME pair corrects it. Two contradictory rows would make
    // the training set ambiguous with no principled way to choose.
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs/acme/labels",
            &steward_key,
            Body::from(
                serde_json::json!({
                    "rec_a_key": "billing:B001",
                    "rec_b_key": "crm:C009",
                    "verdict": "no_match",
                })
                .to_string(),
            ),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let corrected = json_body(resp).await;
    assert_eq!(corrected["verdict"], "no_match");
    assert_eq!(
        corrected["created_at"], label["created_at"],
        "a correction keeps the original created_at, so when anyone first \
         looked at this pair is still recoverable"
    );

    let resp = app
        .clone()
        .oneshot(req(
            "GET",
            "/v1/orgs/acme/labels",
            &viewer_key,
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let listed = json_body(resp).await;
    assert_eq!(
        listed["labels"].as_array().map(Vec::len),
        Some(1),
        "the correction replaced the verdict, it did not add a second row: {listed}"
    );

    // Nonsense is refused, each for its own reason.
    for (body, expect) in [
        (
            serde_json::json!({ "rec_a_key": "a:1", "rec_b_key": "b:2", "verdict": "maybe" }),
            "verdict must be",
        ),
        (
            serde_json::json!({ "rec_a_key": "a:1", "rec_b_key": "a:1", "verdict": "match" }),
            "against itself",
        ),
        (
            serde_json::json!({ "rec_a_key": "", "rec_b_key": "b:2", "verdict": "match" }),
            "both required",
        ),
    ] {
        let resp = app
            .clone()
            .oneshot(req(
                "POST",
                "/v1/orgs/acme/labels",
                &steward_key,
                Body::from(body.to_string()),
            ))
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "should refuse {body}"
        );
        let detail = json_body(resp).await["detail"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            detail.contains(expect),
            "refusal should say {expect:?}, said: {detail}"
        );
    }

    // A viewer may read labels and may not write one.
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs/acme/labels",
            &viewer_key,
            Body::from(
                serde_json::json!({ "rec_a_key": "a:1", "rec_b_key": "b:2", "verdict": "match" })
                    .to_string(),
            ),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    // Coverage is split, not just totalled. Add a second verdict so the split
    // is observable rather than trivially one bucket.
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs/acme/labels",
            &steward_key,
            Body::from(
                serde_json::json!({
                    "rec_a_key": "crm:C001",
                    "rec_b_key": "webforms:W001",
                    "verdict": "match",
                })
                .to_string(),
            ),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    // The queue needs the org's config for its thresholds, so publish one.
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs/acme/config/versions",
            "op-secret",
            Body::from(serde_json::json!({ "yaml": DEFAULT_YAML }).to_string()),
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
            "op-secret",
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // No lake is attached, so the queue cannot serve pairs — but it must fail
    // on the LAKE, having already resolved the labels and the config. A 422
    // here would mean the config/label half is broken; the lake half is
    // exercised in the DuckLake tier.
    let resp = app
        .clone()
        .oneshot(req(
            "GET",
            "/v1/orgs/acme/labeling/next",
            &viewer_key,
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::SERVICE_UNAVAILABLE,
        "with no lake attached the queue should fail on the lake, not earlier: {:?}",
        json_body(resp).await
    );

    // ---- per-version evaluation (E10c) --------------------------------------
    // Neither label recorded above is attributable to a version by the time we
    // get here, which makes this phase a sharper test than it looks:
    // `crm:C001/webforms:W001` was never given one, and `billing:B001/crm:C009`
    // WAS labelled under `v0001` with a score of 0.83 — but the correction that
    // followed sent no `model_version`, and the upsert assigns
    // `model_version = EXCLUDED.model_version`, so it is now null. A correction
    // therefore drops a label out of its version's evaluation, which is right:
    // the score 0.83 was shown against the verdict that was replaced, so
    // scoring the new verdict against it would measure the model on a question
    // it was never asked.
    // Both must stay out of the counts below.
    let admin_key = keys("admin").await;
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs/acme/models/versions",
            &admin_key,
            Body::from(
                serde_json::json!({
                    "model_json": MODEL_JSON,
                    "model_version_label": "v0001",
                })
                .to_string(),
            ),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let created = json_body(resp).await;
    let model_name = created["name"].as_str().unwrap().to_string();
    let model_version = created["version"].as_i64().unwrap();

    // Two scored labels under v0001: one the model got right above the 0.95
    // auto-merge threshold, one it got wrong.
    for (a, b, verdict, score) in [
        ("crm:C100", "crm:C101", "match", 0.99),
        ("crm:C200", "crm:C201", "no_match", 0.97),
    ] {
        let resp = app
            .clone()
            .oneshot(req(
                "POST",
                "/v1/orgs/acme/labels",
                &steward_key,
                Body::from(
                    serde_json::json!({
                        "rec_a_key": a,
                        "rec_b_key": b,
                        "verdict": verdict,
                        "model_version": "v0001",
                        "score_at_label": score,
                    })
                    .to_string(),
                ),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED, "labelling {a}/{b}");
    }

    let eval_path =
        format!("/v1/orgs/acme/models/{model_name}/versions/{model_version}/evaluation");
    let resp = app
        .clone()
        .oneshot(req("GET", &eval_path, &viewer_key, Body::empty()))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    // Both labels predicted a match at 0.95; one verdict disagrees.
    assert_eq!(body["counts"]["labels_for_version"], serde_json::json!(2));
    assert_eq!(body["counts"]["evaluable"], serde_json::json!(2));
    assert_eq!(body["overall"]["tp"], serde_json::json!(1));
    assert_eq!(body["overall"]["fp"], serde_json::json!(1));
    assert_eq!(body["overall"]["precision"], serde_json::json!(0.5));
    assert_eq!(body["threshold"], serde_json::json!(0.95));
    assert_eq!(body["model_version_label"], serde_json::json!("v0001"));
    // The earlier unversioned labels are excluded by the version filter, not
    // merely unscored — they never appear in the denominator at all.
    assert!(
        body["by_origin"]["human"]["tp"] == serde_json::json!(1),
        "human-origin split missing: {body}"
    );

    // Raising the threshold past both scores leaves no positive prediction, so
    // precision is null rather than 0.0 — "made no calls", not "got them wrong".
    let resp = app
        .clone()
        .oneshot(req(
            "GET",
            &format!("{eval_path}?threshold=0.999"),
            &viewer_key,
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let strict = json_body(resp).await;
    assert!(strict["overall"]["precision"].is_null(), "{strict}");

    // A threshold outside [0,1] is refused at the API rather than producing a
    // matrix nothing could act on.
    let resp = app
        .clone()
        .oneshot(req(
            "GET",
            &format!("{eval_path}?threshold=4"),
            &viewer_key,
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);

    // An unknown version is a 404, not an empty evaluation that reads as "this
    // model scored nothing".
    let resp = app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/v1/orgs/acme/models/{model_name}/versions/99/evaluation"),
            &viewer_key,
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    // The role boundary: evaluation is Viewer+, so an unauthenticated caller
    // gets 401 and never sees a tenant's accuracy.
    let resp = app
        .clone()
        .oneshot(req("GET", &eval_path, "not-a-key", Body::empty()))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    let _ = std::fs::remove_file(&config_path);
}
