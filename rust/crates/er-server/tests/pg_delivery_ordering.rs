//! Two extract reports can precede both ingestion jobs. Cursor advancement
//! must leave each queued consumer bound to the bytes its report published.
use er_dag::{ControlCtx, ControlTask, Outcome, Plan, WorkOrder};
use er_server::dags::control::AdvanceSfCursor;
use er_server::dags::plans::ExtractReport;
use er_server::{delivery, pg, runctx};
use serde_json::json;

use crate::common;

const CONFIG: &str = include_str!("../../er-cli/tests/fixtures/test.yaml");

async fn advance(
    client: &tokio_postgres::Client,
    order: &WorkOrder,
    report: ExtractReport,
) -> String {
    let mut ctx = ControlCtx { order, client };
    let result = AdvanceSfCursor
        .run(&mut ctx, (Plan(report),))
        .await
        .unwrap_or_else(|error| match error {
            er_dag::TaskError::Failed(error) => panic!("control task failed: {error}"),
            er_dag::TaskError::Skip => panic!("control task skipped"),
        });
    let Outcome::Done(receipt, _) = result else {
        panic!("nonempty extract enqueues a consumer")
    };
    receipt.0.enqueued_job.unwrap()
}

#[tokio::test]
async fn extracts_advance_only_after_pinned_enqueue_and_both_batches_land_once() {
    let Some(db) = common::fresh_db("delivery_ordering").await else {
        return;
    };
    let mut client = pg::connect(db.dsn()).await.unwrap();
    pg::ensure_schema(&mut client).await.unwrap();
    let root = std::env::temp_dir().join(format!("er-extract-order-{}", er_ids::new_v7()));
    std::fs::create_dir_all(&root).unwrap();
    let config_path = root.join("config.yaml");
    std::fs::write(&config_path, CONFIG).unwrap();
    pg::ensure_org(&client, "delivery-order", config_path.to_str().unwrap())
        .await
        .unwrap();
    let org = pg::get_org(&client, "delivery-order")
        .await
        .unwrap()
        .unwrap();
    let ds = pg::default_dataset_id(&client, &org.id)
        .await
        .unwrap()
        .unwrap();
    let config = er_config::load_config_str(CONFIG).unwrap();
    let spec = &config.sources["crm"];
    let env = json!({"ER_CONFIG_PATH":config_path, "ER_TF_PATH":root.join("tf.csv")});
    let models = [runctx::ActiveModelSpec {
        name: "people".into(),
        version: 1,
        model_json: "{}".into(),
        model_version_label: "v1".into(),
        tf_snapshot_id: Some("tf1".into()),
    }];
    let order: WorkOrder = serde_json::from_value(json!({
        "org_id":org.id, "dataset_id":ds, "job_id":er_ids::new_v7(),
        "run_id":"extract-run", "task_id":"advance_sf_cursor", "kind":"extract", "try_number":1,
        "resource_class":null, "segment":null, "input":null, "params":{}, "lake_env":env,
        "drop_root":root,
    }))
    .unwrap();
    let mut reports = Vec::new();
    for (record, watermark) in [("A", "2026-01-01T00:00:00Z"), ("B", "2026-01-02T00:00:00Z")] {
        let id = er_ids::new_v7();
        let csv = format!(
            "crm_id,last_modified,first_name,last_name\n{record},{watermark},{record},Person\n"
        );
        std::fs::write(root.join(format!(".crm.{id}.csv.part")), &csv).unwrap();
        let pin = delivery::publish(&root, &ds, "crm", &id, spec).unwrap();
        reports.push((
            ExtractReport {
                delivery: Some(pin),
                connection_id: "salesforce-connection".into(),
                source: "crm".into(),
                object: "Contact".into(),
                delivery_id: id,
                mode: "incremental".into(),
                rows: 1,
                bytes: i64::try_from(csv.len()).unwrap(),
                watermark: Some(watermark.into()),
            },
            csv,
        ));
    }
    // A report written by an old pod cannot advance a cursor over an unpinned file.
    let mut old = reports[0].0.clone();
    old.delivery = None;
    assert!(
        AdvanceSfCursor
            .run(
                &mut ControlCtx {
                    order: &order,
                    client: &client
                },
                (Plan(old),)
            )
            .await
            .is_err()
    );
    assert!(
        er_entities::sf_extract_cursor::read::get(&client, &ds, "crm")
            .await
            .unwrap()
            .is_empty()
    );
    let a = advance(&client, &order, reports[0].0.clone()).await;
    assert_eq!(
        advance(&client, &order, reports[0].0.clone()).await,
        a,
        "control retry reuses the consumer"
    );
    let b = advance(&client, &order, reports[1].0.clone()).await;
    assert_ne!(a, b);
    let cursors = er_entities::sf_extract_cursor::read::get(&client, &ds, "crm")
        .await
        .unwrap();
    assert_eq!(cursors[0].delivery_id, reports[1].0.delivery_id);
    // Both cursor writes precede either consumer. No source.csv exists.
    assert!(!root.join("crm.csv").exists());
    let lake = if std::env::var("DEDUPE_E2E_DUCKLAKE").as_deref() == Ok("1") {
        Some(
            er_cli::open_lake(
                root.join("catalog.ducklake").to_str().unwrap(),
                root.join("data").to_str().unwrap(),
            )
            .unwrap(),
        )
    } else {
        eprintln!("skipping delivery lake assertions (set DEDUPE_E2E_DUCKLAKE=1)");
        None
    };
    for (id, csv) in [
        (&a, &reports[0].1),
        (&b, &reports[1].1),
        (&a, &reports[0].1),
    ] {
        let job = pg::get_job(&client, &org.id, id).await.unwrap().unwrap();
        let resolved = runctx::RunContext::resolve(&runctx::ResolveRequest {
            lake_env: &env,
            params: &job.params,
            models: &models,
            drop_root: root.to_str(),
            dataset_id: Some(&ds),
            wants_deliveries: true,
        })
        .unwrap();
        assert_eq!(resolved.deliveries, vec![("crm".into(), csv.clone())]);
        if let Some(lake) = &lake {
            let model = resolved.model();
            let inputs = er_cli::PipelineInputs::full(
                &[],
                &[],
                &resolved.deliveries,
                &model,
                id,
                "2026-01-03 00:00:00",
            );
            er_cli::stage_ingest(lake, &resolved.config, &inputs).unwrap();
        }
    }
    if let Some(lake) = &lake {
        assert_eq!(
            lake.query_strings(
                "select source_record_id, count(*) from lake.main.raw_records group by 1 order by 1"
            )
            .unwrap(),
            vec![vec!["A", "1"], vec!["B", "1"]]
        );
        let receipts = lake.query_strings("select run_id, new_count, changed_count, unchanged_count from lake.main.ingest_batches order by run_id").unwrap();
        let mut expected = vec![
            vec![a, "1".into(), "0".into(), "0".into()],
            vec![b, "1".into(), "0".into(), "0".into()],
        ];
        expected.sort();
        assert_eq!(
            receipts, expected,
            "one correct receipt per consumer despite control and ingest retries"
        );
    }
    drop(lake);
    std::fs::remove_dir_all(root).unwrap();
}
