//! `run_all_full` as SEVEN pods (batch-orchestration-design §11 Phase 3),
//! gated on `ER_TEST_PG_DSN` + `DEDUPE_E2E_DUCKLAKE=1`.
//!
//! The acceptance this file exists for: the DAG path and the composed path
//! produce the SAME lake. Each stage runs in its own `er-server exec-task`
//! child process — its own lake session, its own work-order fetch, its own
//! `RunContext` resolution — and the corpus that comes out is asserted against
//! the very fixture `er-cli/tests/cli_e2e.rs` asserts the monolithic
//! `er run-all` against: base_10's ten entities.
//!
//! If the two ever disagree, the executor is not a second way to run the
//! pipeline — it is a second pipeline, which is the thing §11 forbids.
//!
//! One `#[tokio::test]` with sequential phases (`pg_serial.rs`).

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;

use er_server::executor::{Executor, ExecutorConfig};
use er_server::{AppState, dispatcher, pg};

use crate::common;
use crate::executor_harness::pump;
use crate::fake_pod_api::FakePodApi;

const TEST_YAML: &str = include_str!("../../er-cli/tests/fixtures/test.yaml");
const LEXICON: &str = include_str!("../../er-cli/tests/fixtures/nickname_variants.csv");

fn repo_fixture(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../fixtures/static")
        .join(rel)
}

#[tokio::test]
async fn run_all_full_runs_as_seven_pods_and_reproduces_base_10() {
    let Some(db) = common::fresh_db("run_all_pipeline").await else {
        return;
    };
    if std::env::var("DEDUPE_E2E_DUCKLAKE").as_deref() != Ok("1") {
        eprintln!("skipping the pipeline e2e (set DEDUPE_E2E_DUCKLAKE=1 to run)");
        return;
    }
    let dsn = db.dsn().to_string();
    let mut client = pg::connect(&dsn).await.unwrap();
    pg::ensure_schema(&mut client).await.unwrap();

    // The tenant: a real lake, the same config/model/tf/lexicon the CLI gate
    // uses, and base_10's base delivery waiting in the drop directory.
    let tmp = std::env::temp_dir().join(format!("er_dag_pipeline_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let drop = tmp.join("drop");
    std::fs::create_dir_all(tmp.join("data")).unwrap();
    std::fs::create_dir_all(&drop).unwrap();
    let config_path = tmp.join("acme.yaml");
    std::fs::write(&config_path, TEST_YAML).unwrap();
    let lexicon_path = tmp.join("lexicon.csv");
    std::fs::write(&lexicon_path, LEXICON).unwrap();
    let tf_path = tmp.join("model_test_v1.tf.csv");
    std::fs::copy(repo_fixture("model_test_v1.tf.csv"), &tf_path).unwrap();
    for source in ["crm", "billing", "webforms"] {
        std::fs::copy(
            repo_fixture(&format!("base_10/base/{source}.csv")),
            drop.join(format!("{source}.csv")),
        )
        .unwrap();
    }
    let env = serde_json::json!({
        "ER_LAKE_CATALOG": tmp.join("catalog.ducklake").to_string_lossy(),
        "ER_LAKE_DATA_PATH": tmp.join("data").to_string_lossy(),
        "ER_CONFIG_PATH": config_path.to_string_lossy(),
        "ER_LEXICON_PATH": lexicon_path.to_string_lossy(),
        "ER_MODEL_PATH": repo_fixture("model_test_v1.json").to_string_lossy(),
        // A COPY of the TF fixture, never the fixture itself: `train` APPENDS
        // its snapshot to the shared frozen-TF file, so pointing a tenant at
        // a committed file would have the test mutate the repo. It did, once.
        "ER_TF_PATH": tf_path.to_string_lossy(),
        "ER_MODEL_VERSION": "v0001",
        "ER_TF_SNAPSHOT_ID": "01JWMDTFSNAP00000000000001",
    });
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
    let org_id = pg::org_id_by_name(&client, "acme")
        .await
        .unwrap()
        .expect("org");

    // The real control plane, so the pods fetch real work orders.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = er_server::app(AppState::with_pg("op-secret".to_string(), dsn.clone()));
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let (api, mut frames) = FakePodApi::new(format!("http://{addr}"), tmp.clone());
    let api = Arc::new(api);
    let cfg = ExecutorConfig {
        // Neither kind here provisions, so no profile is needed.
        profile: Ok(None),
        in_flight_cap: 2,
        template: er_k8s::manifest::REFERENCE_TEMPLATE.to_string(),
        runner_orgs: std::collections::BTreeSet::new(),
        image_digest: None,
    };
    let mut executor = Executor::connect(&dsn, Arc::clone(&api) as Arc<dyn er_k8s::PodApi>, cfg)
        .await
        .unwrap();
    executor.adopt().await;

    // `upsert_org_byo` deliberately does NOT converge a default dataset — the
    // sweep's own test needs an unconverged org — so this fixture converges
    // one the way boot does, and runs the job inside it. The dataset's env is
    // the org's verbatim, which is the lake this test wrote.
    pg::converge_default_datasets(&mut client).await.unwrap();
    let ds = common::default_dataset(&client, "acme").await;
    let queued = pg::enqueue(
        &client,
        &org_id,
        Some(&ds),
        "run_all_full",
        0,
        None,
        None,
        pg::Provenance::default(),
    )
    .await
    .unwrap();
    assert!(
        dispatcher::dispatch_once(&client, Some(&mut executor))
            .await
            .unwrap(),
        "the executor claims and plans a run_all_full"
    );
    let rows = er_entities::task_instance::read::by_job(&client, &org_id, &queued.id)
        .await
        .unwrap();
    assert_eq!(
        rows.len(),
        7,
        "init, ingest, standardize, match, reconcile, assemble, finish"
    );

    // Seven child processes, one stage each. Generous: this runs the whole
    // pipeline, and each pod pays a cold DuckDB open.
    let mut settled = String::new();
    for _ in 0..900 {
        pump(&mut executor, &mut frames).await;
        settled = pg::get_job(&client, &org_id, &queued.id)
            .await
            .unwrap()
            .map(|j| j.state)
            .unwrap_or_default();
        if settled != "running" && settled != "dispatching" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    let rows = er_entities::task_instance::read::by_job(&client, &org_id, &queued.id)
        .await
        .unwrap();
    if settled != "succeeded" {
        for r in rows.iter().filter(|r| r.live == Some(true)) {
            eprintln!(
                "  {} {} class={:?} class_err={:?} detail={:?}",
                r.task_id, r.state, r.resource_class, r.error_class, r.error_detail
            );
        }
    }
    assert_eq!(settled, "succeeded", "the whole pipeline settled");

    // Every stage ran as its own pod, each with ONE launch.
    for row in rows.iter().filter(|r| r.live == Some(true)) {
        assert_eq!(row.state, "succeeded", "task {}", row.task_id);
        assert_eq!(row.launches, 1, "{}: one launch", row.task_id);
        assert!(
            row.k8s_job_name
                .as_deref()
                .is_some_and(|n| n.starts_with("er-t-")),
            "{}: a real derived Job name",
            row.task_id
        );
        assert!(row.output.is_some(), "{}: a durable output", row.task_id);
    }
    // `init` published the plan every later stage consumed, and the mode
    // decision it made is the one the ledger records.
    let init = rows
        .iter()
        .find(|r| r.task_id == "init" && r.live == Some(true))
        .expect("the init row");
    let plan = init.output.as_ref().unwrap();
    assert_eq!(plan["plan"], "run_plan");
    assert_eq!(plan["doc"]["mode"], "full");
    assert!(
        plan["doc"]["run_at"]
            .as_str()
            .is_some_and(|s| !s.is_empty()),
        "the plan carries the run's clock"
    );
    // The pods consumed it: `standardize` pinned both the plan and the Landed
    // reference, which only rehydrates if `ingest` published one.
    let std_row = rows
        .iter()
        .find(|r| r.task_id == "standardize" && r.live == Some(true))
        .expect("the standardize row");
    let pins = std_row.input.as_ref().expect("an input")["pins"]
        .as_array()
        .expect("pins")
        .len();
    assert_eq!(pins, 2, "standardize pinned the plan and the landed corpus");

    // ---- the acceptance: the SAME partition the composed path produces -----
    //
    // Read through the DECLARED statement (`entity_membership/read/
    // all_pairs_live`), not freeform SQL: er-server may not name
    // `lake.main.*` itself, and the generated reader is the same one the
    // engine's own reconcile diffs against.
    //
    // SCOPED, and that matters: this handle is a WRITER on the tenant's
    // catalog, and the next phase's pods need it. Holding it across them
    // failed their lake open with a bare "database error" — the
    // one-writer-per-tenant rule (§4.6) biting the harness rather than the
    // product, but the same rule either way.
    let pairs = {
        let session = er_cli::open_lake(
            tmp.join("catalog.ducklake").to_str().unwrap(),
            tmp.join("data").to_str().unwrap(),
        )
        .expect("re-open the lake the pods wrote");
        let pairs = er_entities::entity_membership::read::all_pairs_live(session.connection())
            .expect("pairs");
        let run_rows =
            er_entities::run::read::by_id(session.connection(), &queued.id).expect("run");
        assert_eq!(run_rows.len(), 1, "exactly one run row for this job");
        assert_eq!(run_rows[0].model_version, Some("v0001".to_string()));
        pairs
    };

    // Compare the PARTITION, not the labels: entity ids are minted per run, so
    // what must agree is which records group together. This is the identical
    // comparison `er-cli/tests/cli_e2e.rs` makes against the monolithic
    // `er run-all`, against the same golden file — which is what makes "the
    // two executors agree" a checked claim rather than a hope.
    let mut by_entity: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for r in &pairs {
        by_entity
            .entry(&r.entity_id)
            .or_default()
            .insert(&r.record_key);
    }
    let got: BTreeSet<BTreeSet<&str>> = by_entity.into_values().collect();

    let membership =
        std::fs::read_to_string(repo_fixture("base_10/expected/base/membership.csv")).unwrap();
    let mut want_by: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for line in membership.lines().skip(1).filter(|l| !l.is_empty()) {
        let f: Vec<&str> = line.split(',').collect();
        want_by
            .entry(f[3].to_string())
            .or_default()
            .insert(format!("{}:{}", f[1], f[2]));
    }
    let want: BTreeSet<BTreeSet<String>> = want_by.into_values().collect();
    let got_owned: BTreeSet<BTreeSet<String>> = got
        .iter()
        .map(|g| g.iter().map(|k| (*k).to_string()).collect())
        .collect();
    assert_eq!(
        got_owned.len(),
        10,
        "the DAG path resolves base_10 to ten entities"
    );
    assert_eq!(
        got_owned, want,
        "the SEVEN-POD partition differs from the golden the monolithic path reproduces"
    );

    // ---- phase 2: `train` — the first kind with BOTH halves ----------------
    //
    // Here rather than in its own file because training needs a standardized
    // corpus, and the run above just produced one. Three tasks:
    // `reconcile_schema` and `train` in pods, then `register_model` as a
    // CONTROL task — which runs in-process, because its work is a Postgres
    // write and a pod holds no database credential (§6.4). This is the first
    // test of the executor's control pass.
    let trained = pg::enqueue(
        &client,
        &org_id,
        Some(&ds),
        "train",
        0,
        None,
        None,
        pg::Provenance::default(),
    )
    .await
    .unwrap();
    assert!(
        dispatcher::dispatch_once(&client, Some(&mut executor))
            .await
            .unwrap(),
        "the executor claims and plans a train"
    );
    let mut state = String::new();
    for _ in 0..900 {
        pump(&mut executor, &mut frames).await;
        state = pg::get_job(&client, &org_id, &trained.id)
            .await
            .unwrap()
            .map(|j| j.state)
            .unwrap_or_default();
        if state != "running" && state != "dispatching" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    let rows = er_entities::task_instance::read::by_job(&client, &org_id, &trained.id)
        .await
        .unwrap();
    if state != "succeeded" {
        for r in rows.iter().filter(|r| r.live == Some(true)) {
            eprintln!(
                "  train/{} {} kind={} detail={:?}",
                r.task_id, r.state, r.kind, r.error_detail
            );
        }
    }
    assert_eq!(state, "succeeded", "the train settled");

    // The control task ran as a CONTROL row — never in a pod.
    let register = rows
        .iter()
        .find(|r| r.task_id == "register_model" && r.live == Some(true))
        .expect("the register_model row");
    assert_eq!(register.kind, "control");
    assert_eq!(register.state, "succeeded");
    assert!(
        register.k8s_job_name.is_none(),
        "a control task has no Kubernetes Job: {:?}",
        register.k8s_job_name
    );
    assert_eq!(
        register.output.as_ref().expect("output")["plan"],
        "model_registered"
    );

    // And it wrote a DRAFT registry row, labelled from the job id so it traces
    // back to the training pass that produced it.
    let versions = er_entities::matching_model::read::list(
        &client,
        &common::default_dataset(&client, "acme").await,
    )
    .await
    .unwrap();
    let draft = versions
        .iter()
        .find(|m| m.model_version_label == format!("trained-{}", trained.id))
        .expect("the trained draft is registered");
    assert_eq!(
        draft.state, "draft",
        "a fresh version is never auto-activated"
    );
    assert_eq!(draft.entity_type, "person");
    assert_eq!(draft.provenance["source"], "train");
    assert_eq!(
        draft.provenance["job_id"], trained.id,
        "the draft names the job that trained it"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}
