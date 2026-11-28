//! #667: the cost roll-up against real Postgres, with a fake CUR and a
//! capturing metric sink.
//!
//! Two tenants' attempts on three instances this month and one last month:
//! instance cost splits by wait+run, an attempt the CUR does not cover yet is
//! allocated at the class rate but not billed, rates are calibrated and
//! stored per (class, period), a re-roll replaces rather than duplicates,
//! and the five scalars reach the sink with no tenant in them.
//!
//! ONE test with sequential phases (the file-level rule `pg_serial.rs` checks).

use std::collections::BTreeMap;
use std::sync::Mutex;

use chrono::{DateTime, Duration, TimeZone, Utc};
use er_server::cost_roll::{self, CurSource, MetricSink, Scalars, StatementSource, StatementTime};
use er_server::pg;

use crate::common;

struct FakeCur {
    costs: BTreeMap<String, f64>,
    latest: DateTime<Utc>,
}

impl CurSource for FakeCur {
    fn instance_costs(
        &self,
        _period: &str,
        ids: &[String],
    ) -> impl std::future::Future<Output = Result<BTreeMap<String, f64>, String>> + Send {
        std::future::ready(Ok(ids
            .iter()
            .filter_map(|i| self.costs.get(i).map(|c| (i.clone(), *c)))
            .collect()))
    }
    fn latest_usage_end(
        &self,
    ) -> impl std::future::Future<Output = Result<Option<DateTime<Utc>>, String>> + Send {
        std::future::ready(Ok(Some(self.latest)))
    }
}

#[derive(Default)]
struct Sink(Mutex<Vec<Scalars>>);

impl MetricSink for Sink {
    fn put(&self, s: &Scalars) -> impl std::future::Future<Output = Result<(), String>> + Send {
        self.0.lock().unwrap().push(s.clone());
        std::future::ready(Ok(()))
    }
}

/// Every asked-for run spent half of its `match` stage in one statement and a
/// sliver in another (#1330).
struct FakeStatements;
impl StatementSource for FakeStatements {
    fn durations(
        &self,
        _org: &str,
        _dataset: &str,
        runs: &[String],
    ) -> impl std::future::Future<Output = Result<Vec<StatementTime>, String>> + Send {
        let mut out = Vec::new();
        for run in runs {
            for (statement, ms) in [("match_score.create.batch", 45_000), ("tiny.read.probe", 1)] {
                out.push(StatementTime {
                    run_id: run.clone(),
                    stage: "match".into(),
                    statement: statement.into(),
                    total_ms: ms,
                    executions: 3,
                });
            }
        }
        std::future::ready(Ok(out))
    }
}

#[tokio::test]
async fn runner_cost_is_attributed_by_instance_and_re_rolls_in_place() {
    let Some(db) = common::fresh_db("cost_roll").await else {
        return;
    };
    let mut client = pg::connect(db.dsn()).await.unwrap();
    pg::ensure_schema(&mut client).await.unwrap();
    let router = er_server::app(er_server::AppState::with_pg(
        "op-secret".to_string(),
        db.dsn().to_string(),
    ));
    let mut orgs = Vec::new();
    for name in ["acme", "globex"] {
        use tower::ServiceExt;
        let resp = router
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/v1/orgs")
                    .header("authorization", "Bearer op-secret")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(
                        serde_json::json!({"name": name, "config_path": format!("/tmp/{name}-cost.yaml")}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(resp.status().is_success(), "{}", resp.status());
        let org = pg::get_org(&client, name).await.unwrap().unwrap();
        let ds = pg::default_dataset_id(&client, &org.id)
            .await
            .unwrap()
            .unwrap();
        orgs.push((org.id, ds));
    }
    let now = Utc.with_ymd_and_hms(2026, 11, 20, 12, 0, 0).unwrap();
    let last_month = Utc.with_ymd_and_hms(2026, 10, 15, 12, 0, 0).unwrap();
    // (org index, node, class, state, finished, wait s, run s)
    let attempts = [
        (0, "i-0000000000000a01", "S", "succeeded", now, 30, 90),
        (1, "i-0000000000000a01", "S", "succeeded", now, 0, 240),
        (0, "i-0000000000000a02", "M", "failed", now, 30, 330),
        (0, "i-0000000000000a03", "S", "succeeded", now, 0, 3600),
        (
            0,
            "i-0000000000000b01",
            "S",
            "succeeded",
            last_month,
            0,
            3600,
        ),
    ];
    for (n, (org, node, class, state, finished, wait, run)) in attempts.iter().enumerate() {
        let (org_id, ds) = &orgs[*org];
        let job = pg::enqueue(
            &client,
            org_id,
            Some(ds),
            "run_all_full",
            0,
            None,
            None,
            pg::Provenance::default(),
        )
        .await
        .unwrap();
        let started = *finished - Duration::seconds(*run);
        let queued = started - Duration::seconds(*wait);
        client
            .execute(
                "INSERT INTO task_instances (id, org_id, dataset_id, job_id, task_id, seq, state, kind, live, resource_class, node_name, queued_at, started_at, finished_at) \
                 VALUES ($1, $2, $3, $4, 'match', 0, $5, 'pod', NULL, $6, $7, $8, $9, $10)",
                &[&format!("ti-{n}"), org_id, ds, &job.id, state, class, node, &queued, &started, finished],
            )
            .await
            .unwrap();
    }
    // a03 is not in the CUR yet.
    let cur = FakeCur {
        costs: BTreeMap::from([
            ("i-0000000000000a01".to_string(), 0.36),
            ("i-0000000000000a02".to_string(), 0.10),
            ("i-0000000000000b01".to_string(), 0.20),
        ]),
        latest: now - Duration::hours(30),
    };
    let records = BTreeMap::from([
        (orgs[0].clone(), 1_000_000_i64),
        (orgs[1].clone(), 1_000_000_i64),
    ]);
    let sink = Sink::default();

    let report = cost_roll::roll(&client, &cur, &records, &sink, &FakeStatements, now)
        .await
        .unwrap();
    assert_eq!(report.periods, ["2026-10", "2026-11"]);
    assert_eq!(
        (report.attempts, report.billed_attempts),
        (5, 4),
        "{report:?}"
    );

    let acme = er_entities::run_cost::read::by_org_period(&client, &orgs[0].0, "2026-11")
        .await
        .unwrap();
    let by = |id: &str| {
        acme.iter()
            .find(|c| c.task_instance_id == id)
            .unwrap()
            .clone()
    };
    assert!(
        (by("ti-0").billed_usd.unwrap() - 0.12).abs() < 1e-9,
        "a third of a01's cost"
    );
    assert!((by("ti-2").billed_usd.unwrap() - 0.10).abs() < 1e-9);
    assert_eq!(by("ti-3").billed_usd, None, "not in the CUR yet");
    // S this month: $0.36 over 0.1h → $3.60/h, so a03's hour allocates $3.60.
    assert!((by("ti-3").allocated_usd - 3.6).abs() < 1e-9);
    assert!(
        (by("ti-0").wait_seconds - 30.0).abs() < 1e-9,
        "wait is part of the cost"
    );
    let rates = er_entities::compute_rate::read::for_period(&client, "2026-11")
        .await
        .unwrap();
    let s = rates.iter().find(|r| r.resource_class == "S").unwrap();
    assert!((s.usd_per_pod_hour - 3.6).abs() < 1e-9);
    let october = er_entities::compute_rate::read::for_period(&client, "2026-10")
        .await
        .unwrap();
    assert!((october[0].usd_per_pod_hour - 0.2).abs() < 1e-9);

    // Per-statement spend (#1330): every dataset with attempts this period is
    // reported, its top statement named and priced, nothing under a cent priced
    // alone, and coverage bounded.
    assert!(!report.statements.is_empty(), "{report:?}");
    for d in &report.statements {
        assert_eq!(d.period, "2026-11");
        assert!(d.coverage > 0.0 && d.coverage <= 1.0, "{d:?}");
        assert!(d.attributed_usd <= d.spend_usd + 1e-9, "{d:?}");
        assert!(
            d.top.iter().all(|s| s.usd >= cost_roll::MIN_QUOTED_USD),
            "{d:?}"
        );
        assert!(
            d.top.iter().all(|s| s.statement != "tiny.read.probe"),
            "a sub-cent statement is counted, not priced: {d:?}"
        );
    }
    let acme_statements = report
        .statements
        .iter()
        .find(|d| d.org_id == orgs[0].0)
        .expect("acme's statements");
    assert_eq!(acme_statements.top[0].statement, "match_score.create.batch");
    assert!(
        acme_statements.below_one_cent.statements >= 1,
        "{acme_statements:?}"
    );

    let scalars = sink.0.lock().unwrap().last().cloned().unwrap();
    assert!((scalars.cur_staleness_hours - 30.0).abs() < 1e-9);
    assert!(
        scalars.max_tenant_spend_share > 0.5 && scalars.max_tenant_spend_share < 1.0,
        "{scalars:?}"
    );
    assert!(scalars.failed_run_spend_share > 0.0, "{scalars:?}");
    let json = serde_json::to_string(&scalars).unwrap();
    assert!(
        !json.contains(&orgs[0].0) && !json.contains("acme"),
        "no tenant in a scalar: {json}"
    );

    // --- Re-roll: the CUR now covers a03; rows are replaced, not added -----------
    let cur = FakeCur {
        costs: BTreeMap::from([
            ("i-0000000000000a01".to_string(), 0.36),
            ("i-0000000000000a02".to_string(), 0.10),
            ("i-0000000000000a03".to_string(), 0.30),
            ("i-0000000000000b01".to_string(), 0.20),
        ]),
        latest: now - Duration::hours(2),
    };
    let again = cost_roll::roll(&client, &cur, &records, &sink, &FakeStatements, now)
        .await
        .unwrap();
    assert_eq!(again.billed_attempts, 5);
    let acme = er_entities::run_cost::read::by_org_period(&client, &orgs[0].0, "2026-11")
        .await
        .unwrap();
    assert_eq!(acme.len(), 3, "one row per attempt, replaced in place");
    let a03 = acme.iter().find(|c| c.task_instance_id == "ti-3").unwrap();
    assert!((a03.billed_usd.unwrap() - 0.30).abs() < 1e-9);
    assert!(a03.billed_as_of.is_some());
    assert_eq!(
        er_entities::compute_rate::read::for_period(&client, "2026-11")
            .await
            .unwrap()
            .len(),
        2,
        "S and M, one row each"
    );
}
