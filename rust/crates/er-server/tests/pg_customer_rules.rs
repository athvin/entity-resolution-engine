//! #515: the customer rule surface through the public contract. A config
//! using every advanced rule block — cleanse transforms with a named
//! dictionary, hard constraints, a cross-field comparison group, a segment and
//! a survivorship group — validates, stores and publishes with a tenant admin
//! key; each malformed rule is refused BEFORE a version exists, with a message
//! naming what to change.
//!
//! ONE test with sequential phases (the file-level rule `pg_serial.rs` checks).
//!   ER_TEST_PG_DSN='host=localhost port=5432 user=er password=er dbname=er' \
//!     cargo test -p er-server --test main pg_customer_rules

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use er_server::{AppState, app, pg};
use serde_json::{Value, json};
use tower::ServiceExt;

use crate::common;

const BASE: &str = include_str!("../../er-cli/tests/fixtures/test.yaml");

/// Every advanced block, each in a form a customer would write.
const RULES: &str = r#"
replacement_dictionaries:
  street_types:
    Street: St
    Avenue: Ave
transforms:
  phone:
    - op: regex_replace
      pattern: "[^0-9+]"
      replacement: ""
  address_line:
    - op: named_dictionary
      dictionary: street_types
      mode: words
    - op: trim
constraints:
  veto: [birth_date]
comparison_groups:
  contact_points:
    attributes: [email, phone_e164]
    levels: [exact]
segments:
  west: "addr_region in ('CA','OR','WA')"
survivorship_groups:
  full_name:
    fields: [given_name, family_name]
"#;

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

#[tokio::test]
async fn advanced_customer_rules_validate_publish_and_refuse_with_reasons() {
    let Some(db) = common::fresh_db("customer_rules").await else {
        return;
    };
    let dsn = db.dsn().to_string();
    let mut client = pg::connect(&dsn).await.unwrap();
    pg::ensure_schema(&mut client).await.unwrap();
    let router = app(AppState::with_pg("op-secret".to_string(), dsn.clone()));
    let tmp = std::env::temp_dir().join(format!("er_rules_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let (status, org) = call(
        &router,
        "POST",
        "/v1/orgs",
        "op-secret",
        Some(json!({"name": "acme", "config_path": tmp.join("config.yaml").to_string_lossy()})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{org}");
    let (_, issued) = call(
        &router,
        "POST",
        "/v1/orgs/acme/api-keys",
        "op-secret",
        Some(json!({"role": "admin"})),
    )
    .await;
    let admin = issued["key"].as_str().unwrap().to_owned();
    let ds = "/v1/orgs/acme/datasets/default";
    let validate = |yaml: String| {
        let router = router.clone();
        let admin = admin.clone();
        async move {
            let (status, v) = call(
                &router,
                "POST",
                "/v1/orgs/acme/datasets/default/config/validate",
                &admin,
                Some(json!({ "yaml": yaml })),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{v}");
            v
        }
    };

    // --- 1. Every advanced block together is a valid, publishable config ----
    // A survivorship group is decided once, under its own name, so its members'
    // own chains give way to one `full_name` chain.
    let base = BASE
        .replace(
            "  given_name:  [source_priority, frequency, completeness]\n",
            "",
        )
        .replace(
            "  family_name: [source_priority, frequency, completeness]\n",
            "  full_name:   [source_priority, frequency, completeness]\n",
        );
    assert!(
        base.contains("full_name:   ["),
        "the fixture's chains moved"
    );
    let full = format!("{base}{RULES}");
    let ok = validate(full.clone()).await;
    assert_eq!(ok["valid"], true, "{ok}");
    let (status, draft) = call(
        &router,
        "POST",
        &format!("{ds}/config/versions"),
        &admin,
        Some(json!({"yaml": full})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{draft}");
    let (status, published) = call(
        &router,
        "POST",
        &format!("{ds}/config/versions/{}/publish", draft["version"]),
        &admin,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{published}");
    // Versioned and immutable: the published document round-trips verbatim,
    // and its hash is the one validation reported.
    let (_, active) = call(&router, "GET", &format!("{ds}/config"), &admin, None).await;
    assert_eq!(active["config_hash"], ok["config_hash"], "{active}");
    // A rule edit is classified: transforms change standardization, so C.
    let edited = full.replace("- op: trim", "- op: lower");
    let tiered = validate(edited).await;
    assert_eq!(tiered["changed_blocks"], json!(["transforms"]), "{tiered}");
    assert_eq!(tiered["tier"], "C", "{tiered}");

    // --- 2. Each malformed rule is refused with what to change --------------
    for (mistake, yaml, needle) in [
        (
            "an unparseable regex",
            full.replace(r#"pattern: "[^0-9+]""#, r#"pattern: "([0-9""#),
            "regex_replace pattern",
        ),
        (
            "a transform on a non-attribute",
            full.replace(
                "  phone:\n    - op: regex_replace",
                "  phone_number:\n    - op: regex_replace",
            ),
            "transform on `phone_number` is not a canonical attribute",
        ),
        (
            "a constraint on an unknown column",
            full.replace("veto: [birth_date]", "veto: [birthday]"),
            "constraint column `birthday`",
        ),
        (
            "a dictionary that does not exist",
            full.replace("dictionary: street_types", "dictionary: road_types"),
            "road_types",
        ),
        (
            "a survivorship group over an unknown field",
            full.replace(
                "fields: [given_name, family_name]",
                "fields: [given_name, surname]",
            ),
            "surname",
        ),
    ] {
        let refused = validate(yaml).await;
        assert_eq!(refused["valid"], false, "{mistake}: {refused}");
        assert!(
            refused["problems"].to_string().contains(needle),
            "{mistake}: expected {needle:?} in {refused}"
        );
    }
    // Refusals stored nothing: still exactly one version.
    let (_, versions) = call(
        &router,
        "GET",
        &format!("{ds}/config/versions"),
        &admin,
        None,
    )
    .await;
    assert_eq!(
        versions.as_array().map_or(0, Vec::len) + versions["items"].as_array().map_or(0, Vec::len),
        1,
        "{versions}"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}
