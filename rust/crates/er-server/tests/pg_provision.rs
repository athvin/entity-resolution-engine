//! Tenant auto-provisioning (D8) over a real Postgres (behind `ER_TEST_PG_DSN`):
//! the `POST /v1/orgs` auto branch, the `provision` job that finishes the work,
//! and the unconfigured 503 that was the only behaviour before this. Run with:
//!   ER_TEST_PG_DSN='host=localhost port=5433 user=er password=er dbname=er' \
//!     cargo test -p er-server --test pg_provision
//!
//! ONE `#[tokio::test]` with sequential phases, like every other `pg_*` file:
//! the Postgres tier shares one control-plane schema, so two tests that each
//! drop it deadlock against each other through the `orgs_history` trigger
//! (`pg_serial.rs` enforces this).
//!
//! `local` mode only. The `postgres` mode's extra step is `CREATE DATABASE`,
//! which needs a maintenance connection with createdb rights and leaves a
//! database behind on the test server; its path derivation is covered by unit
//! tests in `provision.rs`, and the job arm it shares with local mode is
//! exercised here.

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use er_server::provision::{Mode, Profile, ProfileError};
use er_server::{AppState, app, pg};
use tower::ServiceExt;

const TEST_YAML: &str = include_str!("../../er-cli/tests/fixtures/test.yaml");

fn op(method: &str, uri: &str, body: Body) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, "Bearer op-secret")
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
async fn provisioning_a_tenant_end_to_end() {
    let Ok(dsn) = std::env::var("ER_TEST_PG_DSN") else {
        eprintln!("skipping provisioning e2e (set ER_TEST_PG_DSN to run)");
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

    let root = std::env::temp_dir().join(format!("er_provision_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let template = root.join("starter.yaml");
    std::fs::write(&template, TEST_YAML).unwrap();

    // ---- phase 1: unconfigured is still the 503 -------------------------
    //
    // The behaviour before D8, and the reason the existing `pg_orgs`
    // assertion stays meaningful: it now tests the UNCONFIGURED path rather
    // than an unimplemented one. An operator who has not opted in sees no
    // change at all.
    //
    // Profiles are constructed directly rather than through the environment:
    // `std::env::set_var` is `unsafe` in edition 2024 and this workspace denies
    // `unsafe_code`, which is why `AppState` carries the profile at all. The env
    // PARSING is covered by `provision.rs`'s own unit tests.
    let unconfigured = app(AppState::with_pg("op-secret".to_string(), dsn.clone()));
    let resp = unconfigured
        .oneshot(op("POST", "/v1/orgs", Body::from(r#"{"name":"nope"}"#)))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::SERVICE_UNAVAILABLE,
        "with no ER_PROVISION_MODE the auto branch must still refuse"
    );

    // ---- phase 2: switched on but misconfigured names the variable ------
    //
    // Worth its own phase because the alternative — treating a broken profile
    // as "provisioning is off" — would have an operator who configured it
    // staring at a 503 with nothing to fix.
    let broken = app(AppState::with_pg("op-secret".to_string(), dsn.clone())
        .with_provision(Err(ProfileError::Missing("ER_PROVISION_ROOT"))));
    let resp = broken
        .oneshot(op("POST", "/v1/orgs", Body::from(r#"{"name":"nope2"}"#)))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let detail = json_body(resp).await["detail"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        detail.contains("ER_PROVISION_ROOT"),
        "the refusal should name the missing variable, got {detail:?}"
    );

    // ---- phase 3: the real thing ----------------------------------------
    let router = app(
        AppState::with_pg("op-secret".to_string(), dsn.clone()).with_provision(Ok(Some(Profile {
            mode: Mode::Local,
            root: root.clone(),
            config_template: template.clone(),
            maint_dsn: None,
            data_template: None,
            env_extra: serde_json::from_str(r#"{"ER_DUCKDB_THREADS":"2"}"#).unwrap(),
        }))),
    );
    let resp = router
        .clone()
        .oneshot(op("POST", "/v1/orgs", Body::from(r#"{"name":"acme"}"#)))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let created = json_body(resp).await;

    // The response shape the BFF and the Playwright mock already agree on.
    assert_eq!(created["name"], "acme");
    assert_eq!(created["state"], "provisioning");
    assert_eq!(created["config_version"], 1);
    assert!(created["job_id"].as_str().is_some(), "{created}");
    assert!(created["admin_key_id"].as_str().is_some(), "{created}");
    // The one-time admin key: a real key, returned once. The UI asserts the
    // `erk_` prefix, which is how a customer knows what they were handed.
    let admin_key = created["admin_key"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        admin_key.starts_with("erk_"),
        "the one-time admin key must be a real key, got {admin_key:?}"
    );
    // Local mode has no catalog database, so no `tenant`.
    assert!(created["tenant"].is_null(), "{created}");

    // ...and that key WORKS, which is the half a shape assertion would miss: a
    // customer handed a key that cannot authenticate has been handed nothing.
    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/orgs/acme")
                .header(header::AUTHORIZATION, format!("Bearer {admin_key}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "the returned admin key must authenticate against its own org"
    );

    // The org is NOT usable yet: provisioning, with its lake still to build.
    let org = pg::get_org(&client, "acme").await.unwrap().unwrap();
    assert_eq!(org.state, "provisioning");
    // Derived paths, with the operator's extra env kept alongside.
    let env = &org.env;
    assert_eq!(
        env.get("ER_LAKE_CATALOG").and_then(|v| v.as_str()),
        Some(
            root.join("acme/lake/catalog.ducklake")
                .to_string_lossy()
                .as_ref()
        )
    );
    assert_eq!(
        env.get("ER_DUCKDB_THREADS").and_then(|v| v.as_str()),
        Some("2"),
        "ER_PROVISION_ENV_JSON should be merged in"
    );
    // config_path and env.ER_CONFIG_PATH agree — the D5 invariant, which
    // provisioning has to satisfy like any other caller.
    assert_eq!(
        env.get("ER_CONFIG_PATH").and_then(|v| v.as_str()),
        Some(root.join("acme/config.yaml").to_string_lossy().as_ref())
    );

    // The starter config is on disk and published as version 1, so the upload
    // path and the config studio have an active config from the start.
    assert!(root.join("acme/config.yaml").exists());
    assert!(root.join("acme/drop").is_dir());
    assert_eq!(org.active_config_version, Some(1));

    // A suspended-or-provisioning org refuses work (D6), which is what keeps a
    // half-built tenant from being handed jobs.
    let resp = router
        .clone()
        .oneshot(op(
            "POST",
            "/v1/orgs/acme/jobs",
            Body::from(r#"{"kind":"run_all_full"}"#),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::CONFLICT,
        "a provisioning tenant must not accept jobs yet"
    );

    // ---- phase 4: a replay is idempotent --------------------------------
    //
    // `POST /v1/orgs` is an upsert and the provision job carries
    // `provision:<org>` as its idempotency key, so a retried request must not
    // produce a second tenant or a second job.
    let resp = router
        .clone()
        .oneshot(op("POST", "/v1/orgs", Body::from(r#"{"name":"acme"}"#)))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let jobs = pg::list_jobs(&client, &org.id).await.unwrap();
    let provisions = jobs.iter().filter(|j| j.kind == "provision").count();
    assert_eq!(
        provisions, 1,
        "the idempotency key should collapse a replayed provision, got {provisions}"
    );

    // ---- phase 5: the provision job builds the lake at the catalog version --
    //
    // Needs the ducklake extension (the lake is opened to create its tables),
    // which is why this phase is gated like the lake tiers are. The dispatcher's
    // profile normally comes from the process environment, which a test cannot
    // set, so the job arm is driven with the same profile the request used.
    if std::env::var("DEDUPE_E2E_DUCKLAKE").as_deref() == Ok("1") {
        let profile = Ok(Some(Profile {
            mode: Mode::Local,
            root: root.clone(),
            config_template: template.clone(),
            maint_dsn: None,
            data_template: None,
            env_extra: serde_json::from_str(r#"{"ER_DUCKDB_THREADS":"2"}"#).unwrap(),
        }));
        let ran = er_server::dispatcher::dispatch_once_with(&client, &profile)
            .await
            .unwrap();
        assert!(ran, "the provision job should have been claimed");
        let org = pg::get_org(&client, "acme").await.unwrap().unwrap();
        assert_eq!(org.state, "active", "provisioning promotes the org");
        // The lake exists with every DDL-owned relation and a ledger that says
        // which catalog version created them.
        let catalog = org.env["ER_LAKE_CATALOG"].as_str().unwrap();
        let data = org.env["ER_LAKE_DATA_PATH"].as_str().unwrap();
        let session = er_cli::open_lake(catalog, data).unwrap();
        assert!(session.lake_table_exists("raw_records").unwrap());
        assert!(session.lake_table_exists("schema_migrations").unwrap());
        let ledger =
            er_entities::lake_schema_migration::read::latest(session.connection()).unwrap();
        assert_eq!(
            ledger.first().map(|r| r.catalog_hash.as_str()),
            Some(er_server::schema::lake_catalog_hash().as_str())
        );
        // ...and the control plane cached the position.
        let states = er_entities::tenant_schema_state::read::list(&client)
            .await
            .unwrap();
        let state = states.iter().find(|s| s.org_id == org.id).unwrap();
        assert_eq!(state.status, "migrated");
        assert_eq!(state.catalog_hash, er_server::schema::lake_catalog_hash());
    } else {
        eprintln!("skipping the provision-job phase (set DEDUPE_E2E_DUCKLAKE=1 to run)");
    }

    let _ = std::fs::remove_dir_all(&root);
}
