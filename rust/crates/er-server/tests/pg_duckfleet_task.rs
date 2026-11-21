//! Actual HTTP task backend through normal job submission and exec-task children.
//! Requires the disposable Compose harness; no production endpoint is accepted.

use std::path::PathBuf;
use std::sync::Arc;

use er_server::executor::{Executor, ExecutorConfig};
use er_server::{AppState, dispatcher, pg};
use serde_json::json;

use crate::{common, executor_harness::pump, fake_pod_api::FakePodApi};

fn fixture(path: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../fixtures/static")
        .join(path)
}

#[tokio::test]
async fn account_golden_runs_remotely_through_the_normal_job_lifecycle() {
    let Ok(address) = std::env::var("ER_DUCKFLEET_PROOF_ADDRESS") else {
        return;
    };
    let address: std::net::SocketAddr = address.parse().unwrap();
    assert!(address.ip().is_loopback());
    let Some(db) = common::fresh_db("duckfleet_task").await else {
        panic!("remote proof requires Postgres")
    };
    let catalog = std::env::var("ER_PROOF_LAKE_CATALOG").unwrap();
    let data_path = "s3://duckfleet/lake/";
    let token = std::env::var("ER_PROOF_TOKEN").unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let config_path = tmp.path().join("account.yaml");
    std::fs::copy(fixture("acct_base/config.yaml"), &config_path).unwrap();
    let drop = tmp.path().join("drop");
    std::fs::create_dir(&drop).unwrap();
    for source in ["crm", "vendors"] {
        std::fs::copy(
            fixture(&format!("acct_base/{source}.csv")),
            drop.join(format!("{source}.csv")),
        )
        .unwrap();
    }
    let env = json!({
        "ER_LAKE_CATALOG": catalog, "ER_LAKE_DATA_PATH":data_path,
        "ER_CONFIG_PATH":config_path, "ER_MODEL_PATH":fixture("acct_model_v1.json"),
        "ER_TF_PATH":fixture("acct_model_v1.tf.csv"), "ER_MODEL_VERSION":"acct_v1",
        "ER_TF_SNAPSHOT_ID":"ACCTTFSNAP0000000000000001"
    });
    let dsn = db.dsn().to_owned();
    let mut client = pg::connect(&dsn).await.unwrap();
    pg::ensure_schema(&mut client).await.unwrap();
    pg::upsert_org_byo(
        &client,
        "acme",
        config_path.to_str().unwrap(),
        &env,
        drop.to_str(),
        None,
        None,
    )
    .await
    .unwrap();
    pg::converge_default_datasets(&mut client).await.unwrap();
    let org = pg::org_id_by_name(&client, "acme").await.unwrap().unwrap();
    let dataset = common::default_dataset(&client, "acme").await;
    let namespace = er_duckfleet::namespace(&org, &dataset);
    let credentials = tmp.path().join("credentials");
    std::fs::create_dir_all(credentials.join(&namespace)).unwrap();
    std::fs::write(
        credentials.join(&namespace).join("credential.json"),
        json!({
            "org_id":org,"dataset_id":dataset,"generation":1,"token":token
        })
        .to_string(),
    )
    .unwrap();
    let registry = tmp.path().join("registry.json");
    std::fs::write(&registry, json!({"bucket":"duckfleet","bindings":[{
        "org_id":org,"dataset_id":dataset,"generation":1,"state":"active","valid_until":4102444800_u64
    }]}).to_string()).unwrap();
    let policy = tmp.path().join("policy.json");
    std::fs::write(&policy, json!({"registry":registry,"credentials":credentials,"statement_seconds":120,
        "classes":{"S":{"mode":"persistent"},"M":{"mode":"persistent"},"L":{"mode":"persistent"}}
    }).to_string()).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let control = format!("http://{}", listener.local_addr().unwrap());
    let app = er_server::app(AppState::with_pg("op-secret".into(), dsn.clone()));
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    let (api, mut frames) = FakePodApi::new(control.clone(), tmp.path().to_path_buf());
    let api = Arc::new(api);
    let template = er_k8s::manifest::REFERENCE_TEMPLATE.replace("          terminationMessagePath:",
        &format!("          env:\n            - name: ER_DUCKFLEET_TASK_POLICY\n              value: {}\n          terminationMessagePath:", serde_json::to_string(&policy).unwrap()));
    let mut executor = Executor::connect(
        &dsn,
        Arc::clone(&api) as Arc<dyn er_k8s::PodApi>,
        ExecutorConfig {
            profile: Ok(None),
            in_flight_cap: 2,
            template,
            runner_orgs: [org.clone()].into_iter().collect(),
            image_digest: None,
        },
    )
    .await
    .unwrap();
    executor.adopt().await;
    let http = reqwest::Client::new();
    let submitted = http
        .post(format!("{control}/v1/orgs/acme/datasets/default/jobs"))
        .bearer_auth("op-secret")
        .json(&json!({"kind":"run_all_full"}))
        .send()
        .await
        .unwrap();
    assert_eq!(submitted.status(), reqwest::StatusCode::ACCEPTED);
    let submitted: serde_json::Value = submitted.json().await.unwrap();
    let job = submitted["id"].as_str().unwrap();
    assert!(
        dispatcher::dispatch_once(&client, Some(&mut executor))
            .await
            .unwrap()
    );
    let waiting: serde_json::Value = http
        .post(format!("{control}/v1/orgs/acme/datasets/default/jobs"))
        .bearer_auth("op-secret")
        .json(&json!({"kind":"run_all_full"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let waiting_id = waiting["id"].as_str().unwrap();
    assert!(
        !dispatcher::dispatch_once(&client, Some(&mut executor))
            .await
            .unwrap(),
        "the dataset already has an active writer"
    );
    assert_eq!(
        pg::get_job(&client, &org, waiting_id)
            .await
            .unwrap()
            .unwrap()
            .state,
        "queued"
    );
    assert!(
        http.post(format!(
            "{control}/v1/orgs/acme/datasets/default/jobs/{waiting_id}/cancel"
        ))
        .bearer_auth("op-secret")
        .send()
        .await
        .unwrap()
        .status()
        .is_success()
    );
    let mut state = String::new();
    for _ in 0..900 {
        pump(&mut executor, &mut frames).await;
        state = pg::get_job(&client, &org, job)
            .await
            .unwrap()
            .unwrap()
            .state;
        if state != "running" && state != "dispatching" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    let tasks = er_entities::task_instance::read::by_job(&client, &org, job)
        .await
        .unwrap();
    if state != "succeeded" {
        for task in &tasks {
            eprintln!("{}: {} {:?}", task.task_id, task.state, task.error_detail);
        }
    }
    assert_eq!(state, "succeeded");
    assert_eq!(
        tasks.iter().filter(|task| task.live == Some(true)).count(),
        8
    );
    let golden = tasks
        .iter()
        .find(|task| task.task_id == "golden" && task.live == Some(true))
        .unwrap();
    assert_eq!(golden.state, "succeeded");
    assert_eq!(golden.launches, 1);
    let history: serde_json::Value = http
        .get(format!("http://{address}/v1/queries?limit=100"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let remote_queries: Vec<_> = history["queries"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|query| query["context"]["er"]["attempt_id"] == golden.id)
        .collect();
    assert!(
        remote_queries.len() >= 8,
        "the golden publication must run on the actual worker"
    );
    for query in &remote_queries {
        assert_eq!(query["context"]["er"]["job_id"], job);
        assert_eq!(query["context"]["er"]["task_id"], "golden");
        assert!(query["query_id"].as_str().is_some());
    }
    let plan = tasks
        .iter()
        .find(|task| task.task_id == "init")
        .unwrap()
        .output
        .as_ref()
        .unwrap();
    let run_at = plan["doc"]["run_at"].as_str().unwrap();
    let session = er_cli::open_lake(&catalog, data_path).unwrap();
    let records = session
        .query_strings(
            "select * exclude(assembled_at) from lake.main.acct_golden_records order by id",
        )
        .unwrap();
    let lineage = session
        .query_strings("select * from lake.main.acct_golden_lineage order by all")
        .unwrap();
    let scores = session
        .query_strings("select * from lake.main.match_scores order by all")
        .unwrap();
    assert_eq!(records.len(), 4);
    let config =
        er_config::load_config_str(&std::fs::read_to_string(&config_path).unwrap()).unwrap();
    er_cli::run_golden(&session, &config, run_at).unwrap();
    assert_eq!(
        session
            .query_strings(
                "select * exclude(assembled_at) from lake.main.acct_golden_records order by id"
            )
            .unwrap(),
        records
    );
    assert_eq!(
        session
            .query_strings("select * from lake.main.acct_golden_lineage order by all")
            .unwrap(),
        lineage
    );
    assert_eq!(
        session
            .query_strings("select * from lake.main.match_scores order by all")
            .unwrap(),
        scores
    );
    std::mem::drop(session);
    // Re-run through the public clear operation, preserving the original plan's
    // logical time. Then remove capacity and prove a terminal task failure.
    for expected in ["succeeded", "failed"] {
        if expected == "failed" {
            std::fs::write(&policy, json!({"registry":registry,"credentials":credentials,"statement_seconds":120,"classes":{}}).to_string()).unwrap();
        }
        assert!(
            http.post(format!(
                "{control}/v1/orgs/acme/datasets/default/jobs/{job}/tasks/golden/clear"
            ))
            .bearer_auth("op-secret")
            .send()
            .await
            .unwrap()
            .status()
            .is_success()
        );
        let _ = dispatcher::dispatch_once(&client, Some(&mut executor))
            .await
            .unwrap();
        let mut final_state = String::new();
        for _ in 0..600 {
            pump(&mut executor, &mut frames).await;
            final_state = pg::get_job(&client, &org, job)
                .await
                .unwrap()
                .unwrap()
                .state;
            if final_state == expected {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
        assert_eq!(final_state, expected);
        let session = er_cli::open_lake(&catalog, data_path).unwrap();
        assert_eq!(
            session
                .query_strings(
                    "select * exclude(assembled_at) from lake.main.acct_golden_records order by id"
                )
                .unwrap(),
            records
        );
        assert_eq!(
            session
                .query_strings("select * from lake.main.acct_golden_lineage order by all")
                .unwrap(),
            lineage
        );
        assert_eq!(
            session
                .query_strings("select * from lake.main.match_scores order by all")
                .unwrap(),
            scores
        );
    }
    println!(
        "{}",
        json!({"job_id":job,"golden_attempt_id":golden.id,"golden_records":records.len(),"golden_lineage":lineage.len(),"matching_scores":scores.len(),"embedded_remote_equal":true,"remote_queries":remote_queries.len(),"writer_exclusion":true,"remote_rerun_equal":true,"unsupported_capacity_job_failed":true})
    );
}
