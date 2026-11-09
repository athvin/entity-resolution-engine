//! The projected-ServiceAccount-token credential through the REAL attempt
//! routes (batch-orchestration-design §6.4): a verified pod token binds to its
//! attempt by the Job-name prefix and OVERRIDES the body's pod identity; a
//! token for another launch, a tampered token, and a JWT presented to a server
//! with no key set are each refused; and the fallback token keeps working
//! beside it.
//!
//! One `#[tokio::test]` with sequential phases (`pg_serial.rs`).

use axum::body::Body;
use axum::http::{Request, StatusCode};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use serde_json::{Value, json};
use tower::ServiceExt;

use er_server::dags::{self, JobKind};
use er_server::satoken::{AUDIENCE, BoxFuture, JwksSource, Verifier};
use er_server::{AppState, app, pg, taskpg};

use crate::common;

mod rotation;

const EPOCH: &str = "epoch-sat";
const TEST_PEM: &str = include_str!("../testdata/sa_token_test_rsa.pem");
const TEST_JWKS: &str = include_str!("../testdata/sa_token_test_jwks.json");

struct FixedJwks;

impl JwksSource for FixedJwks {
    fn fetch(&self) -> BoxFuture<'_, Result<String, String>> {
        Box::pin(async { Ok(TEST_JWKS.to_string()) })
    }
}

fn mint(pod_name: &str, pod_uid: &str) -> String {
    mint_with_key(pod_name, pod_uid, "test-kid-1", TEST_PEM)
}

fn mint_with_key(pod_name: &str, pod_uid: &str, kid: &str, pem: &str) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some(kid.to_string());
    let claims = json!({
        "aud": [AUDIENCE],
        "exp": now + 600,
        "iat": now - 5,
        "nbf": now - 5,
        "sub": "system:serviceaccount:er-dev-test:er-runner",
        "kubernetes.io": {
            "namespace": "er-dev-test",
            "pod": { "name": pod_name, "uid": pod_uid },
            "serviceaccount": { "name": "er-runner", "uid": "sa-uid" },
        },
    });
    encode(
        &header,
        &claims,
        &EncodingKey::from_rsa_pem(pem.as_bytes()).expect("test key"),
    )
    .expect("mint")
}

fn req(method: &str, path: &str, token: &str, body: Option<Value>) -> Request<Body> {
    let b = Request::builder()
        .method(method)
        .uri(path)
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json");
    match body {
        Some(v) => b.body(Body::from(v.to_string())).unwrap(),
        None => b.body(Body::empty()).unwrap(),
    }
}

#[tokio::test]
async fn the_projected_token_binds_overrides_and_refuses() {
    let Some(db) = common::fresh_db("satoken").await else {
        return;
    };
    let dsn = db.dsn().to_string();
    let mut client = pg::connect(&dsn).await.unwrap();
    pg::ensure_schema(&mut client).await.unwrap();
    pg::ensure_org(&client, "acme", "/tmp/acme.yaml")
        .await
        .unwrap();
    let org_id: String = client
        .query_one("SELECT id FROM orgs WHERE name = 'acme'", &[])
        .await
        .unwrap()
        .get(0);

    // A planned lake_maintain with reconcile_schema claimed for launch: the
    // row then carries the derived Job name a pod would extend.
    let dag = dags::dag_for(JobKind::LakeMaintain);
    let job = pg::enqueue(
        &client,
        &org_id,
        None,
        "lake_maintain",
        0,
        None,
        None,
        pg::Provenance::default(),
    )
    .await
    .unwrap();
    let claimed = pg::claim(&client, 1).await.unwrap().expect("claimable");
    taskpg::plan(&mut client, &claimed, &dag, EPOCH, None)
        .await
        .unwrap()
        .expect("plannable");
    let schema_row = taskpg::attempt_id(&job.id, "reconcile_schema", 1);
    er_entities::task_instance::update::mark_scheduled(&client, &json!({}), &schema_row, 0)
        .await
        .unwrap();
    let fallback_token = "fallback-secret-1";
    let launched = taskpg::claim_launch(
        &client,
        EPOCH,
        None,
        Some(&pg::hash_secret(fallback_token)),
        10,
    )
    .await
    .unwrap()
    .expect("launchable");
    assert_eq!(launched.id, schema_row);
    let job_name = launched.k8s_job_name.clone().expect("derived in SQL");
    assert!(job_name.starts_with("er-t-"));

    let verified = app(AppState::with_pg("op-secret".to_string(), dsn.clone())
        .with_sa_verifier(Some(Verifier::new(Box::new(FixedJwks)))));
    let keyless = app(AppState::with_pg("op-secret".to_string(), dsn.clone()));

    rotation::exercise(&dsn, &schema_row, &job_name).await;

    // ---- phase 1: a bound pod token works, and its claims win ------------
    let pod_token = mint(&format!("{job_name}-x7b2k"), "uid-from-claims");
    let resp = verified
        .clone()
        .oneshot(req(
            "GET",
            &format!("/v1/internal/attempts/{schema_row}"),
            &pod_token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "the work order serves");

    // The body LIES about the pod identity; the verified claims override it
    // (§6.3 step 3: identity from the token, never the body).
    let resp = verified
        .clone()
        .oneshot(req(
            "POST",
            &format!("/v1/internal/attempts/{schema_row}/start"),
            &pod_token,
            Some(json!({
                "pod_name": "liar-pod",
                "pod_uid": "liar-uid",
                "k8s_job_name": job_name,
            })),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let row = er_entities::task_instance::read::retry_lineage(&client, &schema_row)
        .await
        .unwrap()
        .remove(0);
    assert_eq!(row.state, "running");
    assert_eq!(row.pod_uid.as_deref(), Some("uid-from-claims"));
    assert_eq!(
        row.pod_name.as_deref(),
        Some(format!("{job_name}-x7b2k").as_str()),
        "pod identity came from the claims, not the lying body"
    );

    // The heartbeat fence also keys on the claims' uid.
    let resp = verified
        .clone()
        .oneshot(req(
            "POST",
            &format!("/v1/internal/attempts/{schema_row}/heartbeat"),
            &pod_token,
            Some(json!({ "pod_uid": "liar-uid" })),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "fenced by the claims' uid");

    // ---- phase 2: the refusals -------------------------------------------
    // A token whose pod extends a DIFFERENT launch: `…-l1` must not admit an
    // `…-l10` pod (the trailing-dash rule).
    let cousin = mint(&format!("{job_name}0-x7b2k"), "uid-cousin");
    let resp = verified
        .clone()
        .oneshot(req(
            "GET",
            &format!("/v1/internal/attempts/{schema_row}"),
            &cousin,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // A tampered token (another pod's payload under this signature).
    let minted = mint(&format!("{job_name}-x7b2k"), "uid-from-claims");
    let other = mint(&format!("{job_name}-x7b2k"), "uid-forged");
    let (h, s) = (
        minted.split('.').next().unwrap(),
        minted.split('.').nth(2).unwrap(),
    );
    let forged = format!("{h}.{}.{s}", other.split('.').nth(1).unwrap());
    let resp = verified
        .clone()
        .oneshot(req(
            "GET",
            &format!("/v1/internal/attempts/{schema_row}"),
            &forged,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // A JWT presented to a server with no key set is refused outright — it is
    // never hashed against the fallback column.
    let resp = keyless
        .clone()
        .oneshot(req(
            "GET",
            &format!("/v1/internal/attempts/{schema_row}"),
            &pod_token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // ---- phase 3: the fallback token still works beside it ----------------
    let resp = verified
        .clone()
        .oneshot(req(
            "POST",
            &format!("/v1/internal/attempts/{schema_row}/finish"),
            fallback_token,
            Some(json!({
                "state": "succeeded",
                "pod_uid": "uid-from-claims",
                "outcome": "done",
                "exit_code": 0,
            })),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "the fallback token finishes the same attempt"
    );
    let row = er_entities::task_instance::read::retry_lineage(&client, &schema_row)
        .await
        .unwrap()
        .remove(0);
    assert_eq!(row.state, "succeeded");
    assert!(row.token_hash.is_none(), "nulled at finish");
}
