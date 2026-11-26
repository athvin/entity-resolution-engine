//! #1010: a tenant admin configures a dataset of its own through the public
//! contract — create, validate a draft, publish — and the publish lands on
//! THAT dataset, not the org's default one.
//!
//! Before #1010 the publish handler read its context off the org row, so a
//! publish to any second dataset overwrote the default dataset's config file,
//! classified its tier against the default's document and repointed the org;
//! `GET …/config` read the same org pointer, so every dataset was served the
//! default's config. Phase 3 pins both.
//!
//! ONE test with sequential phases (the file-level rule `pg_serial.rs` checks).
//!   ER_TEST_PG_DSN='host=localhost port=5432 user=er password=er dbname=er' \
//!     cargo test -p er-server --test main pg_dataset_config

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use er_server::provision::{Mode, Profile};
use er_server::{AppState, app, pg};
use serde_json::{Value, json};
use tower::ServiceExt;

use crate::common;

const YAML: &str = include_str!("../../er-cli/tests/fixtures/test.yaml");

async fn call(
    router: &axum::Router,
    method: &str,
    uri: &str,
    key: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header(header::AUTHORIZATION, format!("Bearer {key}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn draft_and_publish(
    router: &axum::Router,
    key: &str,
    base: &str,
    yaml: &str,
) -> (i64, Value) {
    let (status, created) = call(
        router,
        "POST",
        &format!("{base}/config/versions"),
        key,
        Some(json!({ "yaml": yaml })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let version = created["version"].as_i64().unwrap();
    let (status, published) = call(
        router,
        "POST",
        &format!("{base}/config/versions/{version}/publish"),
        key,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{published}");
    (version, published)
}

#[tokio::test]
async fn a_tenant_admin_configures_its_own_dataset() {
    let Some(db) = common::fresh_db("dataset_config").await else {
        return;
    };
    let dsn = db.dsn().to_string();
    let mut client = pg::connect(&dsn).await.unwrap();
    pg::ensure_schema(&mut client).await.unwrap();

    let root = std::env::temp_dir().join(format!("er_dataset_config_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let template = root.join("starter.yaml");
    std::fs::write(&template, YAML).unwrap();
    let profile = Profile {
        mode: Mode::Local,
        root: root.clone(),
        config_template: template,
        maint_dsn: None,
        data_template: None,
        env_extra: serde_json::Map::new(),
    };
    let router =
        app(AppState::with_pg("op-secret".to_string(), dsn.clone())
            .with_provision(Ok(Some(profile))));

    // --- Setup: a BYO org (active at once) and its keys ---------------------
    let default_path = root.join("default.yaml");
    let (status, created) = call(
        &router,
        "POST",
        "/v1/orgs",
        "op-secret",
        Some(json!({ "name": "acme", "config_path": default_path.to_string_lossy() })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let mut keys = std::collections::BTreeMap::new();
    for role in ["admin", "viewer"] {
        let (status, issued) = call(
            &router,
            "POST",
            "/v1/orgs/acme/api-keys",
            "op-secret",
            Some(json!({ "role": role })),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{issued}");
        keys.insert(role, issued["key"].as_str().unwrap().to_owned());
    }
    let admin = keys["admin"].clone();
    let viewer = keys["viewer"].clone();
    let (default_v, _) =
        draft_and_publish(&router, &admin, "/v1/orgs/acme/datasets/default", YAML).await;

    // --- 1. Dataset lifecycle through the tenant's own admin key ------------
    // Auto-provisioned: the server derives the lake and seeds the starter.
    let (status, provisioning) = call(
        &router,
        "POST",
        "/v1/orgs/acme/datasets",
        &admin,
        Some(json!({ "name": "web", "display_name": "Web leads", "entity_type": "person" })),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{provisioning}");
    assert_eq!(provisioning["state"], "provisioning", "{provisioning}");
    assert!(provisioning["job_id"].as_str().is_some(), "{provisioning}");
    let (status, web) = call(&router, "GET", "/v1/orgs/acme/datasets/web", &admin, None).await;
    assert_eq!(status, StatusCode::OK, "{web}");
    assert_eq!(web["display_name"], "Web leads");
    // Operator-managed paths stay operator power.
    let (status, refused) = call(
        &router,
        "POST",
        "/v1/orgs/acme/datasets",
        &admin,
        Some(json!({ "name": "byo", "config_path": "/etc/passwd" })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{refused}");
    // A viewer may not create; another org's admin cannot see this one.
    let (status, _) = call(
        &router,
        "POST",
        "/v1/orgs/acme/datasets",
        &viewer,
        Some(json!({ "name": "nope" })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // A dataset still provisioning refuses a publish rather than racing it.
    let (status, draft) = call(
        &router,
        "POST",
        "/v1/orgs/acme/datasets/web/config/versions",
        &admin,
        Some(json!({ "yaml": YAML })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{draft}");
    let (status, refused) = call(
        &router,
        "POST",
        &format!(
            "/v1/orgs/acme/datasets/web/config/versions/{}/publish",
            draft["version"]
        ),
        &admin,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");

    // A second, active dataset with its own file (BYO, so it is active now).
    let second_path = root.join("crm2.yaml");
    let (status, created) = call(
        &router,
        "POST",
        "/v1/orgs/acme/datasets",
        "op-secret",
        Some(json!({ "name": "crm2", "config_path": second_path.to_string_lossy() })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let base = "/v1/orgs/acme/datasets/crm2";

    // --- 2. Validate a draft before anything is stored ----------------------
    let versions_before = call(
        &router,
        "GET",
        &format!("{base}/config/versions"),
        &admin,
        None,
    )
    .await
    .1;
    let validate = |yaml: String| {
        let router = router.clone();
        let admin = admin.clone();
        async move {
            let (status, v) = call(
                &router,
                "POST",
                "/v1/orgs/acme/datasets/crm2/config/validate",
                &admin,
                Some(json!({ "yaml": yaml })),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{v}");
            v
        }
    };
    let ok = validate(YAML.to_owned()).await;
    assert_eq!(ok["valid"], true, "{ok}");
    assert_eq!(ok["tier"], "C", "a first publish rebuilds everything: {ok}");
    assert!(ok["config_hash"].as_str().is_some(), "{ok}");

    // Every mistake at once, each with what to change.
    let broken = YAML
        .replace("email:        email_address", "emial:        email_address")
        .replace("priority_rank: 2", "priority_rank: 1")
        .replace("expr: \"phone_e164\"", "expr: \"phone_number\"");
    let bad = validate(broken).await;
    assert_eq!(bad["valid"], false, "{bad}");
    let problems: Vec<&str> = bad["problems"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p.as_str().unwrap())
        .collect();
    for needle in [
        "maps `emial`, which is not a canonical person attribute",
        "share priority_rank 1",
        "unknown standardized column `phone_number`",
    ] {
        assert!(
            problems.iter().any(|p| p.contains(needle)),
            "{needle}: {problems:?}"
        );
    }
    let unparsable = validate("sources: [".to_owned()).await;
    assert_eq!(unparsable["valid"], false, "{unparsable}");
    assert!(unparsable["config_hash"].is_null(), "{unparsable}");
    let wrong_type = validate(format!("entity_type: account\n{YAML}")).await;
    assert!(
        wrong_type
            .to_string()
            .contains("dataset's entity type is fixed"),
        "{wrong_type}"
    );
    // Validation stored nothing.
    let versions_after = call(
        &router,
        "GET",
        &format!("{base}/config/versions"),
        &admin,
        None,
    )
    .await
    .1;
    assert_eq!(versions_before, versions_after);
    // Admin+ only, and scoped like every dataset route.
    let (status, _) = call(
        &router,
        "POST",
        &format!("{base}/config/validate"),
        &viewer,
        Some(json!({ "yaml": YAML })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = call(
        &router,
        "POST",
        "/v1/orgs/acme/datasets/missing/config/validate",
        &admin,
        Some(json!({ "yaml": YAML })),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // Publish refuses what validate reported, before any job is queued.
    let (status, created) = call(
        &router,
        "POST",
        &format!("{base}/config/versions"),
        &admin,
        Some(
            json!({ "yaml": YAML.replace("given_name:   first_name", "givn_name:   first_name") }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let (status, refused) = call(
        &router,
        "POST",
        &format!("{base}/config/versions/{}/publish", created["version"]),
        &admin,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    assert!(
        refused["detail"].as_str().unwrap().contains("givn_name"),
        "{refused}"
    );

    // --- 3. The publish lands on THIS dataset -------------------------------
    let default_before = std::fs::read_to_string(&default_path).unwrap();
    let edited = YAML.replace("auto_merge: 0.95", "auto_merge: 0.97");
    let (crm2_v, published) = draft_and_publish(&router, &admin, base, &edited).await;
    assert_eq!(published["tier"], "C", "first publish of crm2: {published}");
    assert!(
        std::fs::read_to_string(&second_path)
            .unwrap()
            .contains("auto_merge: 0.97"),
        "crm2's own file carries its config"
    );
    assert_eq!(
        std::fs::read_to_string(&default_path).unwrap(),
        default_before,
        "the default dataset's file is untouched"
    );
    let (_, active) = call(&router, "GET", &format!("{base}/config"), &admin, None).await;
    assert_eq!(active["version"], crm2_v, "{active}");
    let (_, default_active) = call(
        &router,
        "GET",
        "/v1/orgs/acme/datasets/default/config",
        &admin,
        None,
    )
    .await;
    assert_eq!(default_active["version"], default_v, "{default_active}");
    let org = pg::get_org(&client, "acme").await.unwrap().unwrap();
    assert_eq!(
        org.active_config_version,
        Some(default_v),
        "the org mirror follows the default dataset only"
    );
    // A second crm2 publish classifies against crm2's config, not the default's:
    // only the thresholds moved, so tier A.
    let validated = validate(YAML.replace("auto_merge: 0.95", "auto_merge: 0.98")).await;
    assert_eq!(validated["tier"], "A", "{validated}");
    assert_eq!(
        validated["changed_blocks"],
        json!(["thresholds"]),
        "{validated}"
    );

    let _ = std::fs::remove_dir_all(&root);
}
