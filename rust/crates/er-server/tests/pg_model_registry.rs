//! The matching-model registry over real Postgres (behind `ER_TEST_PG_DSN`).
//!
//! Before this, a tenant's model was a FILE PATH in its env (`ER_MODEL_PATH`).
//! There was no way to see which model an org was scoring against, no way to
//! change it without editing the org's env, no history of what it used to be,
//! and no way to read the m/u probabilities the engine had always carried —
//! `grep m_probability` over the server and the frontend returned nothing.
//!
//! What these assertions pin down:
//!   * creating an org SEEDS its starter model from the file its env already
//!     names, and activates it — so a tenant created after the registry scores
//!     against byte-identical settings to one created before it;
//!   * re-registering an org does NOT mint a second version (the create endpoint
//!     is an idempotent upsert and versions auto-increment);
//!   * versions are immutable and monotonic, uploads land as drafts, and
//!     activation is a separate explicit step;
//!   * activation is atomic: exactly one version is ever active;
//!   * the weights view exposes each level's m/u and the log-odds they imply;
//!   * a model for the other entity type cannot be activated;
//!   * role boundaries: a viewer reads, only an admin writes.
//!
//! Run with e.g.
//!   ER_TEST_PG_DSN='host=localhost port=5433 user=er password=er dbname=er' \
//!     cargo test -p er-server --test pg_model_registry

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use er_server::{AppState, app, pg};
use tower::ServiceExt;

const DEFAULT_YAML: &str = include_str!("../../er-config/tests/fixtures/default.yaml");

/// The committed person starter model — the same file an org's `ER_MODEL_PATH`
/// points at in the dev stack and in every lake test.
fn starter_model_path() -> String {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../fixtures/static/model_test_v1.json")
        .to_string_lossy()
        .into_owned()
}

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
    let bytes = axum::body::to_bytes(resp.into_body(), 8 << 20)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn the_model_registry_seeds_versions_and_activates_exactly_one() {
    let Ok(dsn) = std::env::var("ER_TEST_PG_DSN") else {
        eprintln!("skipping Postgres model registry (set ER_TEST_PG_DSN to run)");
        return;
    };

    let mut client = pg::connect(&dsn).await.unwrap();
    client
        .batch_execute(
            "DROP TABLE IF EXISTS matching_models, staged_steward_actions, audit_log, schedules, config_versions, api_keys, jobs, orgs CASCADE;",
        )
        .await
        .unwrap();
    pg::ensure_schema(&mut client).await.unwrap();

    let app = app(AppState::with_pg("op-secret".to_string(), dsn.clone()));
    let model_path = starter_model_path();
    let starter_json = std::fs::read_to_string(&model_path).unwrap();
    let config_path = std::env::temp_dir().join(format!("er_models_{}.yaml", std::process::id()));
    let config_path = config_path.to_string_lossy().to_string();
    let _ = std::fs::remove_file(&config_path);

    // The org names its model file and its stamp, exactly as the dev stack does.
    let create_org = serde_json::json!({
        "name": "acme",
        "config_path": config_path,
        "env": {
            "ER_MODEL_PATH": model_path,
            "ER_MODEL_VERSION": "v0001",
            "ER_TF_SNAPSHOT_ID": "01JWMDTFSNAP00000000000001",
        },
    });
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs",
            "op-secret",
            Body::from(create_org.to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let created = json_body(resp).await;
    assert_eq!(
        created["starter_model"]["seeded"], true,
        "creating an org must seed its starter model: {created}"
    );
    assert_eq!(created["starter_model"]["version"], 1);

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

    // A viewer can see what the org scores against.
    let resp = app
        .clone()
        .oneshot(req(
            "GET",
            "/v1/orgs/acme/models",
            &viewer_key,
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let listed = json_body(resp).await;
    assert_eq!(listed["items"].as_array().unwrap().len(), 1);
    assert_eq!(listed["active"]["version"], 1);
    assert_eq!(listed["items"][0]["state"], "active");
    assert_eq!(
        listed["items"][0]["model_version_label"], "v0001",
        "the seeded stamp must be the org's ER_MODEL_VERSION, because the TF \
         snapshot CSV is keyed by it"
    );
    assert_eq!(listed["items"][0]["provenance"]["origin"], "seeded");
    // The list must not carry the settings documents.
    assert!(
        listed["items"][0].get("model_json").is_none(),
        "the list projection must omit model_json"
    );

    // Re-registering the org is an upsert and must NOT mint a second version.
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs",
            "op-secret",
            Body::from(create_org.to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let again = json_body(resp).await;
    assert_eq!(again["starter_model"]["seeded"], false);
    let resp = app
        .clone()
        .oneshot(req(
            "GET",
            "/v1/orgs/acme/models",
            &viewer_key,
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(
        json_body(resp).await["items"].as_array().unwrap().len(),
        1,
        "an org upsert must not accumulate model versions"
    );

    // The weights view: the m/u probabilities the engine always had and nothing
    // could read.
    let resp = app
        .clone()
        .oneshot(req(
            "GET",
            "/v1/orgs/acme/models/default/versions/1",
            &viewer_key,
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let detail = json_body(resp).await;
    assert_eq!(
        detail["model_json"].as_str().unwrap(),
        starter_json,
        "the stored document must be byte-identical to the seeded file"
    );
    let comparisons = detail["weights"]["comparisons"].as_array().unwrap();
    assert_ne!(
        comparisons.len(),
        0,
        "the weights view must expose the model's comparisons"
    );
    let levels = comparisons[0]["levels"].as_array().unwrap();
    assert_ne!(levels.len(), 0);
    assert!(
        levels
            .iter()
            .any(|l| l["m_probability"].is_number() && l["u_probability"].is_number()),
        "at least one level must expose m and u: {levels:?}"
    );
    assert!(
        levels.iter().any(|l| l["log2_bayes_factor"].is_number()),
        "the weights view must render the log-odds a level contributes"
    );
    assert!(
        detail["weights"]["blocking_rules"].is_array(),
        "the weights view must expose the blocking rules"
    );

    // Publishing a config so uploads can resolve the org's entity type.
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs/acme/config/versions",
            &admin_key,
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
            &admin_key,
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // A viewer cannot author a model.
    let upload = serde_json::json!({
        "model_json": starter_json,
        "model_version_label": "v0002",
    });
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs/acme/models/versions",
            &viewer_key,
            Body::from(upload.to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    // An unloadable document is refused here rather than inside a run.
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs/acme/models/versions",
            &admin_key,
            Body::from(
                serde_json::json!({ "model_json": "{\"not\":\"a model\"}", "model_version_label": "v0002" })
                    .to_string(),
            ),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let detail = json_body(resp).await["detail"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        detail.contains("not a loadable model"),
        "the refusal should say why: {detail}"
    );

    // A label is required, not guessed: a wrong stamp silently voids the TF
    // lookup rather than failing, so the caller has to state it.
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs/acme/models/versions",
            &admin_key,
            Body::from(serde_json::json!({ "model_json": starter_json }).to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::UNPROCESSABLE_ENTITY,
        "model_version_label must be required"
    );

    // A real upload lands as a DRAFT: authoring a model must not re-cluster.
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs/acme/models/versions",
            &admin_key,
            Body::from(upload.to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let v2 = json_body(resp).await;
    assert_eq!(v2["version"], 2, "versions are monotonic within a name");
    assert_eq!(v2["state"], "draft", "an upload must not activate itself");
    assert_eq!(v2["provenance"]["origin"], "uploaded");

    // v1 is still what runs score against.
    let resp = app
        .clone()
        .oneshot(req(
            "GET",
            "/v1/orgs/acme/models",
            &viewer_key,
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(json_body(resp).await["active"]["version"], 1);

    // A viewer cannot activate.
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs/acme/models/default/versions/2/activate",
            &viewer_key,
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    // Activation moves the active pointer and archives the previous version, in
    // one statement — so there is never a moment with no active model, and never
    // two.
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs/acme/models/default/versions/2/activate",
            &admin_key,
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let resp = app
        .clone()
        .oneshot(req(
            "GET",
            "/v1/orgs/acme/models",
            &viewer_key,
            Body::empty(),
        ))
        .await
        .unwrap();
    let listed = json_body(resp).await;
    assert_eq!(listed["active"]["version"], 2);
    let states: Vec<(i64, String)> = listed["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| {
            (
                m["version"].as_i64().unwrap(),
                m["state"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    assert_eq!(
        states,
        vec![(2, "active".to_string()), (1, "archived".to_string())],
        "activating v2 must archive v1"
    );
    assert_eq!(
        states.iter().filter(|(_, s)| s == "active").count(),
        1,
        "exactly one version may be active"
    );

    // Re-activating the already-active version is a no-op, not a way to end up
    // with zero active models.
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs/acme/models/default/versions/2/activate",
            &admin_key,
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let resp = app
        .clone()
        .oneshot(req(
            "GET",
            "/v1/orgs/acme/models",
            &viewer_key,
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(
        json_body(resp).await["active"]["version"],
        2,
        "re-activating the active version must leave it active"
    );

    // A missing version is a 404, not a silent nothing.
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs/acme/models/default/versions/99/activate",
            &admin_key,
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    // An account model cannot be activated against a person lake: its SQL names
    // columns (company_name_norm, domain_norm) that this lake does not have, and
    // the only symptom would be a failed run. The API has no way to author one
    // for a person org — entity_type comes from the published config, not the
    // caller — so the row is inserted directly to exercise the guard.
    let org_id: String = client
        .query_one("SELECT id FROM orgs WHERE name = 'acme'", &[])
        .await
        .unwrap()
        .get(0);
    client
        .execute(
            "INSERT INTO matching_models
               (id, org_id, name, version, entity_type, model_json, model_hash,
                model_version_label, provenance, created_by)
             VALUES ($1, $2, 'accounts', 1, 'account', $3, 'deadbeef', 'acct-v1',
                     '{}'::jsonb, 'test')",
            &[&er_ids::new_v7(), &org_id, &starter_json],
        )
        .await
        .unwrap();
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs/acme/models/accounts/versions/1/activate",
            &admin_key,
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::CONFLICT,
        "activating an account model against a person org must refuse"
    );
    let detail = json_body(resp).await["detail"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        detail.contains("person"),
        "the refusal should name what the org resolves: {detail}"
    );
    // And the refusal left the active pointer alone.
    let resp = app
        .clone()
        .oneshot(req(
            "GET",
            "/v1/orgs/acme/models",
            &viewer_key,
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(
        json_body(resp).await["active"]["version"],
        2,
        "a refused activation must not disturb the active model"
    );

    // ---- compiling the config into a model ----
    //
    // This is the endpoint that makes the tenant-facing `comparisons:` /
    // `blocking:` blocks mean something: they were authored, validated at
    // publish and hashed, and never read by anything that runs.
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs/acme/models/compile",
            &viewer_key,
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "compiling mints a model version, so it is an admin action"
    );

    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs/acme/models/compile",
            &admin_key,
            Body::empty(),
        ))
        .await
        .unwrap();
    let compile_status = resp.status();
    let compiled = json_body(resp).await;
    assert_eq!(
        compile_status,
        StatusCode::CREATED,
        "compile refused: {}",
        compiled["detail"].as_str().unwrap_or("(no detail)")
    );
    assert_eq!(
        compiled["state"], "draft",
        "compiling must not activate: a config edit cannot be allowed to re-cluster silently"
    );
    assert_eq!(compiled["provenance"]["origin"], "compiled");
    assert_eq!(
        compiled["model_version_label"], "v0001",
        "a compiled model keeps the org's stamp, or the TF lookup would find nothing"
    );
    assert!(
        compiled["notes"].as_array().unwrap().iter().any(|n| n
            .as_str()
            .unwrap_or_default()
            .contains("DECLARED, not estimated")),
        "the response must say the parameters were not trained: {compiled}"
    );

    // The compiled document is a real model the engine loads, and it carries the
    // comparisons the config declared.
    let compiled_version = compiled["version"].as_i64().unwrap();
    let resp = app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/v1/orgs/acme/models/default/versions/{compiled_version}"),
            &viewer_key,
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let detail = json_body(resp).await;
    let columns: Vec<&str> = detail["weights"]["comparisons"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["output_column_name"].as_str().unwrap())
        .collect();
    // The fixture config declares exactly these six comparisons.
    assert_eq!(
        columns,
        vec![
            "addr_postal",
            "birth_date",
            "email",
            "family_name",
            "given_name",
            "phone_e164"
        ],
        "the compiled model must compare what the config declared"
    );
    // And the blocking rules the config declared reached it.
    assert_ne!(
        detail["weights"]["blocking_rules"]
            .as_array()
            .unwrap()
            .len(),
        0,
        "the config's blocking rules must reach the compiled model"
    );

    // Activating it is the same explicit step as any other version.
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            &format!("/v1/orgs/acme/models/default/versions/{compiled_version}/activate"),
            &admin_key,
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let resp = app
        .clone()
        .oneshot(req(
            "GET",
            "/v1/orgs/acme/models",
            &viewer_key,
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(
        json_body(resp).await["active"]["version"],
        compiled_version,
        "the compiled model is now what runs score against"
    );

    let _ = std::fs::remove_file(&config_path);
}
