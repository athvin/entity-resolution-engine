//! Cost attribution (#667): what each runner attempt cost, a self-calibrated
//! price per class, and the five aggregate alarm scalars.
//!
//! # The join
//!
//! EKS Auto Mode names a node by its EC2 instance id, and the CUR keys every
//! instance's lines — `BoxUsage`/`SpotUsage` and the Auto Mode management
//! fee — by the same id in `line_item_resource_id`. A runner pod holds its
//! node alone (the runner taint, `do-not-disrupt`, one task per node), so an
//! instance's whole billed cost belongs to the attempts it hosted, split by
//! their wait-plus-run time. That join needs no split cost allocation data,
//! which the account does not produce (0 of 149,479 November rows, probed
//! 2026-11-27): the split columns would only refine a pod's share of a SHARED
//! node, and runner nodes are not shared.
//!
//! # The numbers
//!
//! - `billed_usd` = instance cost × the attempt's share of the instance's
//!   hosted time — NULL until the CUR covers the instance (24–48h late, and
//!   revised, so every roll recomputes the current and previous period).
//! - `usd_per_pod_hour` per class = billed / attempt-hours over the class's
//!   billed attempts. Self-calibrated, so it folds in the instance family, the
//!   spot mix and the Auto Mode fee.
//! - `allocated_usd` = (wait + run) hours × that rate (the previous period's
//!   rate while the current one has no billed attempts yet). Wait counts: a
//!   node was provisioned for the attempt.
//!
//! S and M carry close rates by design: a 2-vCPU S pod holds a whole node as
//! a 6-vCPU M pod does, so the cost lever is duration, not class.
//!
//! Alarm scalars carry no tenant dimension — the tenant is in the report,
//! never a metric label.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Datelike, TimeZone, Utc};
use serde::Serialize;

/// One finished runner attempt.
#[derive(Debug, Clone, PartialEq)]
pub struct Attempt {
    pub task_instance_id: String,
    pub org_id: String,
    pub dataset_id: Option<String>,
    pub job_id: String,
    pub task_id: String,
    pub resource_class: Option<String>,
    pub node_name: String,
    pub state: String,
    pub wait_seconds: f64,
    pub run_seconds: f64,
}

impl Attempt {
    fn hours(&self) -> f64 {
        (self.wait_seconds + self.run_seconds) / 3600.0
    }
    fn failed(&self) -> bool {
        matches!(self.state.as_str(), "failed" | "timed_out" | "up_for_retry")
    }
}

/// One attempt's cost.
#[derive(Debug, Clone, PartialEq)]
pub struct Costed {
    pub attempt: Attempt,
    pub allocated_usd: f64,
    pub billed_usd: Option<f64>,
}

/// One class's calibrated rate.
#[derive(Debug, Clone, PartialEq)]
pub struct Rate {
    pub resource_class: String,
    pub usd_per_pod_hour: f64,
    pub attempt_hours: f64,
    pub billed_usd: f64,
    pub instances: i64,
}

/// A period's allocation.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Allocation {
    pub costs: Vec<Costed>,
    pub rates: Vec<Rate>,
}

fn class_of(a: &Attempt) -> String {
    a.resource_class.clone().unwrap_or_else(|| "S".to_string())
}

/// Attribute each instance's billed cost to the attempts it hosted, and
/// calibrate a rate per class. `prior` is the previous period's rate per
/// class, used for `allocated_usd` where this period has none yet.
#[must_use]
pub fn allocate(
    attempts: &[Attempt],
    instance_usd: &BTreeMap<String, f64>,
    prior: &BTreeMap<String, f64>,
) -> Allocation {
    let mut hosted: BTreeMap<&str, f64> = BTreeMap::new();
    for a in attempts {
        *hosted.entry(a.node_name.as_str()).or_default() += a.wait_seconds + a.run_seconds;
    }
    let billed: Vec<Option<f64>> = attempts
        .iter()
        .map(|a| {
            let usd = instance_usd.get(&a.node_name)?;
            let total = hosted.get(a.node_name.as_str()).copied().unwrap_or(0.0);
            Some(if total > 0.0 {
                usd * (a.wait_seconds + a.run_seconds) / total
            } else {
                0.0
            })
        })
        .collect();
    // Rates over the billed attempts only.
    let mut by_class: BTreeMap<String, (f64, f64, BTreeSet<&str>)> = BTreeMap::new();
    for (a, b) in attempts.iter().zip(&billed) {
        if let Some(usd) = b {
            let e = by_class.entry(class_of(a)).or_default();
            e.0 += usd;
            e.1 += a.hours();
            e.2.insert(a.node_name.as_str());
        }
    }
    let rates: Vec<Rate> = by_class
        .iter()
        .filter(|(_, (_, hours, _))| *hours > 0.0)
        .map(|(class, (usd, hours, nodes))| Rate {
            resource_class: class.clone(),
            usd_per_pod_hour: usd / hours,
            attempt_hours: *hours,
            billed_usd: *usd,
            instances: i64::try_from(nodes.len()).unwrap_or(i64::MAX),
        })
        .collect();
    let rate_of = |class: &str| {
        rates
            .iter()
            .find(|r| r.resource_class == class)
            .map(|r| r.usd_per_pod_hour)
            .or_else(|| prior.get(class).copied())
            .unwrap_or(0.0)
    };
    let costs = attempts
        .iter()
        .zip(billed)
        .map(|(a, billed_usd)| Costed {
            allocated_usd: a.hours() * rate_of(&class_of(a)),
            billed_usd,
            attempt: a.clone(),
        })
        .collect();
    Allocation { costs, rates }
}

/// The five alarm scalars (no tenant dimension).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Scalars {
    /// The largest single org's share of the period's spend.
    pub max_tenant_spend_share: f64,
    /// max over (org, dataset) of share-of-spend ÷ share-of-records: a small
    /// tenant costing like a big one. 0 when records are unknown.
    pub max_tenant_disproportion: f64,
    /// Spend on attempts that failed, timed out or were retried, ÷ all spend.
    pub failed_run_spend_share: f64,
    /// max over datasets of (cost per million records now ÷ last period) − 1.
    pub max_cost_per_million_mom_change: f64,
    /// Hours since the newest CUR line; the CUR is expected within 48h.
    pub cur_staleness_hours: f64,
}

fn spend(c: &Costed) -> f64 {
    c.billed_usd.unwrap_or(c.allocated_usd)
}

fn by_dataset(costs: &[Costed]) -> BTreeMap<(String, String), f64> {
    let mut out: BTreeMap<(String, String), f64> = BTreeMap::new();
    for c in costs {
        if let Some(ds) = &c.attempt.dataset_id {
            *out.entry((c.attempt.org_id.clone(), ds.clone()))
                .or_default() += spend(c);
        }
    }
    out
}

/// Compute the scalars for `current` (with `previous` for month-over-month),
/// given connected records per (org, dataset).
#[must_use]
pub fn scalars(
    current: &[Costed],
    previous: &[Costed],
    records: &BTreeMap<(String, String), i64>,
    cur_staleness_hours: f64,
) -> Scalars {
    let total: f64 = current.iter().map(spend).sum();
    let mut out = Scalars {
        cur_staleness_hours,
        ..Scalars::default()
    };
    if total <= 0.0 {
        return out;
    }
    let mut per_org: BTreeMap<&str, f64> = BTreeMap::new();
    for c in current {
        *per_org.entry(c.attempt.org_id.as_str()).or_default() += spend(c);
    }
    out.max_tenant_spend_share = per_org.values().fold(0.0, |m, v| m.max(v / total));
    out.failed_run_spend_share = current
        .iter()
        .filter(|c| c.attempt.failed())
        .map(spend)
        .sum::<f64>()
        / total;
    let now_ds = by_dataset(current);
    let all_records: i64 = records.values().sum();
    if all_records > 0 {
        out.max_tenant_disproportion = now_ds
            .iter()
            .filter_map(|(k, usd)| {
                let r = *records.get(k)?;
                (r > 0).then(|| (usd / total) / (r as f64 / all_records as f64))
            })
            .fold(0.0, f64::max);
    }
    let then_ds = by_dataset(previous);
    out.max_cost_per_million_mom_change = now_ds
        .iter()
        .filter_map(|(k, usd)| {
            let r = *records.get(k)? as f64;
            let before = *then_ds.get(k)?;
            (r > 0.0 && before > 0.0).then(|| (usd / r) / (before / r) - 1.0)
        })
        .fold(0.0, f64::max);
    out
}

/// `YYYY-MM` and its [start, end) bounds.
#[must_use]
pub fn period_bounds(at: DateTime<Utc>) -> (String, DateTime<Utc>, DateTime<Utc>) {
    let start = Utc
        .with_ymd_and_hms(at.year(), at.month(), 1, 0, 0, 0)
        .single()
        .unwrap_or(at);
    let (y, m) = if at.month() == 12 {
        (at.year() + 1, 1)
    } else {
        (at.year(), at.month() + 1)
    };
    let end = Utc
        .with_ymd_and_hms(y, m, 1, 0, 0, 0)
        .single()
        .unwrap_or(at);
    (format!("{:04}-{:02}", at.year(), at.month()), start, end)
}

/// The previous period's start (any instant within it).
#[must_use]
pub fn previous_period(at: DateTime<Utc>) -> DateTime<Utc> {
    period_bounds(at).1 - chrono::Duration::days(1)
}

/// An EC2 instance id, checked before it reaches the Athena SQL.
#[must_use]
pub fn valid_instance_id(id: &str) -> bool {
    id.strip_prefix("i-").is_some_and(|hex| {
        (8..=17).contains(&hex.len()) && hex.bytes().all(|b| b.is_ascii_hexdigit())
    })
}

/// Where instance costs come from: Athena over the CUR in production.
pub trait CurSource {
    /// Billed USD per instance id for `period`, over the given ids.
    fn instance_costs(
        &self,
        period: &str,
        instance_ids: &[String],
    ) -> impl std::future::Future<Output = Result<BTreeMap<String, f64>, String>> + Send;
    /// The newest usage end in the CUR, for staleness.
    fn latest_usage_end(
        &self,
    ) -> impl std::future::Future<Output = Result<Option<DateTime<Utc>>, String>> + Send;
}

/// Where the scalars go: CloudWatch in production.
pub trait MetricSink {
    fn put(
        &self,
        scalars: &Scalars,
    ) -> impl std::future::Future<Output = Result<(), String>> + Send;
}

/// What a roll did.
#[derive(Debug, Clone, Default, Serialize)]
pub struct RollReport {
    pub periods: Vec<String>,
    pub attempts: usize,
    pub billed_attempts: usize,
    pub rates: usize,
    pub scalars: Scalars,
}

fn attempt_of(r: &er_entities::task_instance::read::FinishedPodsBetweenRow) -> Option<Attempt> {
    let started = r.started_at?;
    let finished = r.finished_at?;
    let node = r.node_name.clone()?;
    let secs =
        |a: DateTime<Utc>, b: DateTime<Utc>| ((b - a).num_milliseconds().max(0) as f64) / 1000.0;
    Some(Attempt {
        task_instance_id: r.id.clone(),
        org_id: r.org_id.clone(),
        dataset_id: r.dataset_id.clone(),
        job_id: r.job_id.clone(),
        task_id: r.task_id.clone(),
        resource_class: r.resource_class.clone(),
        node_name: node,
        state: r.state.clone(),
        wait_seconds: r.queued_at.map_or(0.0, |q| secs(q, started)),
        run_seconds: secs(started, finished),
    })
}

/// Roll the current and previous period: attribute, persist, emit.
///
/// # Errors
/// A database, CUR or metric failure, as text.
pub async fn roll(
    client: &tokio_postgres::Client,
    cur: &impl CurSource,
    records: &BTreeMap<(String, String), i64>,
    sink: &impl MetricSink,
    now: DateTime<Utc>,
) -> Result<RollReport, String> {
    let mut report = RollReport::default();
    let mut rolled: Vec<Vec<Costed>> = Vec::new();
    // Previous first, so its rates seed the current period's allocation.
    let mut prior: BTreeMap<String, f64> = BTreeMap::new();
    for at in [previous_period(now), now] {
        let (period, start, end) = period_bounds(at);
        let rows = er_entities::task_instance::read::finished_pods_between(client, start, end)
            .await
            .map_err(|e| format!("reading attempts: {e}"))?;
        let attempts: Vec<Attempt> = rows
            .iter()
            .filter_map(attempt_of)
            .filter(|a| valid_instance_id(&a.node_name))
            .collect();
        let ids: Vec<String> = attempts
            .iter()
            .map(|a| a.node_name.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let costs = if ids.is_empty() {
            BTreeMap::new()
        } else {
            cur.instance_costs(&period, &ids).await?
        };
        let allocation = allocate(&attempts, &costs, &prior);
        for r in &allocation.rates {
            er_entities::compute_rate::create::upsert(
                client,
                &er_ids::new_v7(),
                &r.resource_class,
                &period,
                r.usd_per_pod_hour,
                r.attempt_hours,
                r.billed_usd,
                r.instances,
            )
            .await
            .map_err(|e| format!("storing a rate: {e}"))?;
        }
        for c in &allocation.costs {
            let a = &c.attempt;
            er_entities::run_cost::create::upsert(
                client,
                &er_ids::new_v7(),
                &a.org_id,
                a.dataset_id.as_deref(),
                &a.job_id,
                &a.task_instance_id,
                &a.task_id,
                a.resource_class.as_deref(),
                &a.node_name,
                &a.state,
                &period,
                a.wait_seconds,
                a.run_seconds,
                c.allocated_usd,
                c.billed_usd,
                c.billed_usd.map(|_| now),
            )
            .await
            .map_err(|e| format!("storing an attempt's cost: {e}"))?;
        }
        report.attempts += allocation.costs.len();
        report.billed_attempts += allocation
            .costs
            .iter()
            .filter(|c| c.billed_usd.is_some())
            .count();
        report.rates += allocation.rates.len();
        report.periods.push(period);
        prior = allocation
            .rates
            .iter()
            .map(|r| (r.resource_class.clone(), r.usd_per_pod_hour))
            .collect();
        rolled.push(allocation.costs);
    }
    let staleness = cur.latest_usage_end().await?.map_or(f64::INFINITY, |end| {
        ((now - end).num_minutes().max(0) as f64) / 60.0
    });
    let (previous, current) = (&rolled[0], &rolled[1]);
    report.scalars = scalars(current, previous, records, staleness);
    sink.put(&report.scalars).await?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A test attempt; the dataset is the org's own (`o1` → `d1`).
    fn attempt(
        id: &str,
        org: &str,
        node: &str,
        class: &str,
        state: &str,
        wait: f64,
        run: f64,
    ) -> Attempt {
        Attempt {
            task_instance_id: id.into(),
            org_id: org.into(),
            dataset_id: Some(org.replace('o', "d")),
            job_id: format!("job-{id}"),
            task_id: "match".into(),
            resource_class: Some(class.into()),
            node_name: node.into(),
            state: state.into(),
            wait_seconds: wait,
            run_seconds: run,
        }
    }

    #[test]
    fn an_instances_cost_splits_across_its_attempts_by_wait_plus_run() {
        let attempts = vec![
            attempt(
                "a",
                "o1",
                "i-0000000000000001",
                "S",
                "succeeded",
                30.0,
                90.0,
            ),
            attempt(
                "b",
                "o2",
                "i-0000000000000001",
                "S",
                "succeeded",
                0.0,
                240.0,
            ),
            attempt("c", "o1", "i-0000000000000002", "M", "failed", 30.0, 330.0),
            // Not yet in the CUR: allocated from the prior rate, billed NULL.
            attempt(
                "d",
                "o1",
                "i-0000000000000003",
                "L",
                "succeeded",
                0.0,
                3600.0,
            ),
        ];
        let costs = BTreeMap::from([
            ("i-0000000000000001".to_string(), 0.36),
            ("i-0000000000000002".to_string(), 0.10),
        ]);
        let prior = BTreeMap::from([("L".to_string(), 0.50)]);
        let out = allocate(&attempts, &costs, &prior);
        let by = |id: &str| {
            out.costs
                .iter()
                .find(|c| c.attempt.task_instance_id == id)
                .unwrap()
                .clone()
        };
        // a: 120s of the node's 360s → a third; wait counts.
        assert!((by("a").billed_usd.unwrap() - 0.12).abs() < 1e-9);
        assert!((by("b").billed_usd.unwrap() - 0.24).abs() < 1e-9);
        assert!((by("c").billed_usd.unwrap() - 0.10).abs() < 1e-9);
        assert_eq!(by("d").billed_usd, None);
        assert!(
            (by("d").allocated_usd - 0.50).abs() < 1e-9,
            "prior rate × 1h"
        );
        // S: $0.36 over 360s = 0.1h → $3.60/h; allocated a = 120s × that.
        let s = out.rates.iter().find(|r| r.resource_class == "S").unwrap();
        assert!((s.usd_per_pod_hour - 3.6).abs() < 1e-9);
        assert_eq!(s.instances, 1);
        assert!((by("a").allocated_usd - 0.12).abs() < 1e-9);

        let records = BTreeMap::from([
            (("o1".into(), "d1".into()), 1_000_000),
            (("o2".into(), "d2".into()), 1_000_000),
        ]);
        let previous = vec![Costed {
            attempt: attempt("p", "o1", "i-9", "S", "succeeded", 0.0, 1.0),
            allocated_usd: 0.1,
            billed_usd: Some(0.1),
        }];
        let sc = scalars(&out.costs, &previous, &records, 30.0);
        // spend: o1 = 0.12 + 0.10 + 0.50, o2 = 0.24 → total 0.96
        assert!((sc.max_tenant_spend_share - 0.72 / 0.96).abs() < 1e-9);
        assert!((sc.failed_run_spend_share - 0.10 / 0.96).abs() < 1e-9);
        assert!((sc.max_tenant_disproportion - (0.72 / 0.96) / 0.5).abs() < 1e-9);
        assert!((sc.max_cost_per_million_mom_change - (0.72 / 0.1 - 1.0)).abs() < 1e-9);
        assert!((sc.cur_staleness_hours - 30.0).abs() < 1e-9);
    }

    #[test]
    fn periods_and_ids_are_bounded() {
        let at = Utc.with_ymd_and_hms(2026, 12, 15, 0, 0, 0).unwrap();
        let (p, s, e) = period_bounds(at);
        assert_eq!(p, "2026-12");
        assert_eq!(s, Utc.with_ymd_and_hms(2026, 12, 1, 0, 0, 0).unwrap());
        assert_eq!(e, Utc.with_ymd_and_hms(2027, 1, 1, 0, 0, 0).unwrap());
        assert_eq!(period_bounds(previous_period(at)).0, "2026-11");
        assert!(valid_instance_id("i-0851a55f0c462da5f"));
        assert!(!valid_instance_id("i-0851' OR 1=1"));
        assert!(!valid_instance_id("ip-10-0-0-1"));
    }
}
