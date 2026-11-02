//! Links between entities that are NOT merges (E11b), behind `ER_TEST_PG_DSN`:
//! the upsert corrects rather than stacks, the read answers from both sides,
//! the role boundary holds, and a link cannot cross tenants.
//!
//! The property worth stating up front, because it is what the feature is for:
//! the product has exactly one answer to "these two records belong together" —
//! merge them — and that answer is wrong for a husband and wife at one address
//! or a subsidiary and its parent. A merge loses the distinction and no later
//! run recovers it. These tests are about the alternative behaving like a
//! first-class record rather than a note.
//!
//! Run with e.g.
//!   ER_TEST_PG_DSN='host=localhost port=5466 user=er password=er dbname=er' \
//!     cargo test -p er-server --test pg_entity_links

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use er_server::{AppState, app, pg};
use tower::ServiceExt;

use crate::common;

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

/// One test with phases: the Postgres tier shares one control-plane schema, so
/// two tests that each drop it deadlock through the `orgs_history` trigger
/// (`pg_serial.rs` enforces this).
#[tokio::test]
async fn entity_links_upsert_read_and_role_boundary() {
    let Some(db) = common::fresh_db("entity_links").await else {
        return;
    };
    let dsn = db.dsn().to_string();

    let mut client = pg::connect(&dsn).await.unwrap();
    pg::ensure_schema(&mut client).await.unwrap();

    let app = app(AppState::with_pg("op-secret".to_string(), dsn.clone()));
    for name in ["acme", "other"] {
        let resp = app
            .clone()
            .oneshot(req(
                "POST",
                "/v1/orgs",
                "op-secret",
                Body::from(format!(
                    r#"{{"name":"{name}","config_path":"/configs/{name}.yaml","env":{{}}}}"#
                )),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
    }
    let acme_id = pg::org_id_by_name(&client, "acme").await.unwrap().unwrap();
    let other_id = pg::org_id_by_name(&client, "other").await.unwrap().unwrap();

    // `issue_key` returns (key_id, full_key); the bearer is the full key.
    let (_, steward) = pg::issue_key(&client, &acme_id, "steward").await.unwrap();
    let (_, viewer) = pg::issue_key(&client, &acme_id, "viewer").await.unwrap();

    // --- phase 1: a steward links two entities without merging them.
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs/acme/datasets/default/entity-links",
            &steward,
            Body::from(
                r#"{"parent_entity_id":"ent-household-1","child_entity_id":"ent-person-1",
                    "kind":"household","note":"same address, different people"}"#,
            ),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let created = json_body(resp).await;
    assert_eq!(created["kind"], "household");
    assert_eq!(
        created["origin"], "manual",
        "this route mints `manual` and must not take the origin from the body, \
         or a steward's judgement could claim to be a model's suggestion"
    );

    // --- phase 2: re-asserting the same link corrects it rather than stacking.
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs/acme/datasets/default/entity-links",
            &steward,
            Body::from(
                r#"{"parent_entity_id":"ent-household-1","child_entity_id":"ent-person-1",
                    "kind":"household","note":"corrected"}"#,
            ),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let corrected = json_body(resp).await;
    assert_eq!(corrected["note"], "corrected");
    assert_eq!(
        corrected["created_at"], created["created_at"],
        "when the relationship was first recorded must survive a correction"
    );

    // A DIFFERENT kind between the same pair is a different link, not a
    // correction: "these two are a household" and "these two are related" are
    // separate claims, and the natural key includes the kind for that reason.
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs/acme/datasets/default/entity-links",
            &steward,
            Body::from(
                r#"{"parent_entity_id":"ent-household-1","child_entity_id":"ent-person-1",
                    "kind":"related"}"#,
            ),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    // --- phase 3: the read answers from BOTH sides.
    let resp = app
        .clone()
        .oneshot(req(
            "GET",
            "/v1/orgs/acme/datasets/default/entities/ent-person-1/links",
            &viewer,
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let from_child = json_body(resp).await;
    assert_eq!(
        from_child["items"].as_array().unwrap().len(),
        2,
        "the child side must see both links; a read that only matched \
         parent_entity_id would make the entity view answer 'nothing is \
         related to this' for every member of every household"
    );

    let resp = app
        .clone()
        .oneshot(req(
            "GET",
            "/v1/orgs/acme/datasets/default/entities/ent-household-1/links",
            &viewer,
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(
        json_body(resp).await["items"].as_array().unwrap().len(),
        2,
        "and the parent side sees them too"
    );

    // --- phase 4: refusals that would otherwise be silent.
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs/acme/datasets/default/entity-links",
            &steward,
            Body::from(
                r#"{"parent_entity_id":"ent-a","child_entity_id":"ent-a","kind":"household"}"#,
            ),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::UNPROCESSABLE_ENTITY,
        "an entity in its own household is meaningless and nothing downstream \
         would notice"
    );

    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs/acme/datasets/default/entity-links",
            &steward,
            Body::from(r#"{"parent_entity_id":"ent-a","child_entity_id":"ent-b","kind":"spouse"}"#),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::UNPROCESSABLE_ENTITY,
        "an unknown kind must come back naming the alternatives, not as a 500 \
         from the column constraint"
    );

    // --- phase 5: the role boundary. A viewer may read links and must not
    // create them — linking is a steward's judgement.
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/v1/orgs/acme/datasets/default/entity-links",
            &viewer,
            Body::from(
                r#"{"parent_entity_id":"ent-x","child_entity_id":"ent-y","kind":"company"}"#,
            ),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    // --- phase 6: tenant isolation. `other`'s steward must not see or delete
    // acme's links, and the delete is org-scoped in the STATEMENT, so it holds
    // even if a handler check were loosened.
    let (_, other_steward) = pg::issue_key(&client, &other_id, "steward").await.unwrap();
    let resp = app
        .clone()
        .oneshot(req(
            "GET",
            "/v1/orgs/acme/datasets/default/entities/ent-person-1/links",
            &other_steward,
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "404, not 403: `guard` answers a wrong-org key with 'no org' so a \
         probe cannot confirm which tenants exist. The weaker 403 would leak \
         exactly that."
    );

    let link_id = corrected["id"].as_str().unwrap();
    assert_eq!(
        er_entities::entity_link::delete::by_id(&client, &other_id, link_id)
            .await
            .unwrap(),
        0,
        "the statement is org-scoped: another tenant's id must delete nothing"
    );

    // --- phase 7: the owning steward deletes it, and only it.
    let resp = app
        .clone()
        .oneshot(req(
            "DELETE",
            &format!("/v1/orgs/acme/datasets/default/entity-links/{link_id}"),
            &steward,
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app
        .clone()
        .oneshot(req(
            "GET",
            "/v1/orgs/acme/datasets/default/entities/ent-person-1/links",
            &viewer,
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(
        json_body(resp).await["items"].as_array().unwrap().len(),
        1,
        "deleting the household link must leave the `related` one alone"
    );

    let resp = app
        .clone()
        .oneshot(req(
            "DELETE",
            &format!("/v1/orgs/acme/datasets/default/entity-links/{link_id}"),
            &steward,
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "a second delete reports that there was nothing to delete rather than \
         claiming success"
    );
}
