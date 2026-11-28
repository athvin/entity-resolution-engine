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

/// Where a dataset's statement times come from: its lake's
/// `statement_executions` in production (#1330).
pub trait StatementSource {
    /// Each statement's total time per (run, stage), for the given runs.
    fn durations(
        &self,
        org_id: &str,
        dataset_id: &str,
        run_ids: &[String],
    ) -> impl std::future::Future<Output = Result<Vec<StatementTime>, String>> + Send;
}

/// One statement's time within one (run, stage).
#[derive(Debug, Clone, PartialEq)]
pub struct StatementTime {
    pub run_id: String,
    pub stage: String,
    pub statement: String,
    pub total_ms: i64,
    pub executions: i64,
}

/// The smallest per-statement figure the report quotes: below a cent, an
/// attributed share is noise, so it is only ever counted, never priced alone.
pub const MIN_QUOTED_USD: f64 = 0.01;
/// How many statements a dataset's report names.
pub const TOP_STATEMENTS: usize = 10;

/// One statement's share of a dataset's spend.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StatementCost {
    pub statement: String,
    pub usd: f64,
    /// Of the dataset's period spend.
    pub share: f64,
    pub total_ms: i64,
    pub executions: i64,
}

/// Statements counted but not named.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Unnamed {
    pub statements: usize,
    pub usd: f64,
}

/// A dataset's spend by statement for one period (#1330).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DatasetStatements {
    pub org_id: String,
    pub dataset_id: String,
    pub period: String,
    pub spend_usd: f64,
    /// The part of the spend the statements account for.
    pub attributed_usd: f64,
    /// Statement time over attempt wall time, in `[0, 1]`: how much of what was
    /// billed the statements explain. Low coverage means the figures below are
    /// a small, possibly unrepresentative, slice of the bill.
    pub coverage: f64,
    /// Up to [`TOP_STATEMENTS`], by cost, each at least [`MIN_QUOTED_USD`].
    pub top: Vec<StatementCost>,
    /// Statements of at least a cent beyond the top.
    pub rest: Unnamed,
    /// Statements under a cent, never priced individually.
    pub below_one_cent: Unnamed,
}

fn cents(usd: f64) -> f64 {
    (usd * 100.0).round() / 100.0
}

/// Share each (run, stage)'s spend across its statements by duration over the
/// stage's attempt wall time (#1330). An attempt's statements explain at most
/// its wall time, so the remainder (pod start, waits, uninstrumented work)
/// stays unattributed and shows as coverage below 1, rather than inflating the
/// statements. Retries of a stage share the stage's rows: the rows carry the
/// run and stage, not the attempt.
#[must_use]
pub fn statement_costs(
    period: &str,
    org_id: &str,
    dataset_id: &str,
    costs: &[&Costed],
    times: &[StatementTime],
) -> DatasetStatements {
    let mut stages: BTreeMap<(&str, &str), (f64, f64)> = BTreeMap::new();
    for c in costs {
        let entry = stages
            .entry((c.attempt.job_id.as_str(), c.attempt.task_id.as_str()))
            .or_default();
        entry.0 += spend(c);
        entry.1 += c.attempt.run_seconds * 1000.0;
    }
    let mut by_stage: BTreeMap<(&str, &str), Vec<&StatementTime>> = BTreeMap::new();
    for t in times {
        by_stage
            .entry((t.run_id.as_str(), t.stage.as_str()))
            .or_default()
            .push(t);
    }
    let spend_usd: f64 = stages.values().map(|(usd, _)| usd).sum();
    let wall_ms: f64 = stages.values().map(|(_, ms)| ms).sum();
    let mut covered_ms = 0.0;
    let mut per_statement: BTreeMap<&str, (f64, i64, i64)> = BTreeMap::new();
    for (key, (usd, wall)) in &stages {
        let Some(rows) = by_stage.get(key) else {
            continue;
        };
        let total: f64 = rows.iter().map(|t| t.total_ms.max(0) as f64).sum();
        if total <= 0.0 || *wall <= 0.0 {
            continue;
        }
        // More statement time than wall time is clock skew or concurrency; never
        // attribute more than the stage cost.
        let denominator = total.max(*wall);
        covered_ms += total.min(*wall);
        for t in rows {
            let e = per_statement.entry(t.statement.as_str()).or_default();
            e.0 += usd * (t.total_ms.max(0) as f64) / denominator;
            e.1 += t.total_ms;
            e.2 += t.executions;
        }
    }
    let mut ranked: Vec<StatementCost> = per_statement
        .into_iter()
        .map(|(statement, (usd, total_ms, executions))| StatementCost {
            statement: statement.to_string(),
            usd,
            share: if spend_usd > 0.0 {
                usd / spend_usd
            } else {
                0.0
            },
            total_ms,
            executions,
        })
        .collect();
    ranked.sort_by(|a, b| {
        b.usd
            .total_cmp(&a.usd)
            .then_with(|| a.statement.cmp(&b.statement))
    });
    let attributed_usd: f64 = ranked.iter().map(|s| s.usd).sum();
    let (quoted, small): (Vec<_>, Vec<_>) =
        ranked.into_iter().partition(|s| s.usd >= MIN_QUOTED_USD);
    let mut top: Vec<StatementCost> = quoted.iter().take(TOP_STATEMENTS).cloned().collect();
    for s in &mut top {
        s.usd = cents(s.usd);
        s.share = (s.share * 10_000.0).round() / 10_000.0;
    }
    let beyond = &quoted[top.len()..];
    DatasetStatements {
        org_id: org_id.to_string(),
        dataset_id: dataset_id.to_string(),
        period: period.to_string(),
        spend_usd: cents(spend_usd),
        attributed_usd: cents(attributed_usd),
        coverage: if wall_ms > 0.0 {
            ((covered_ms / wall_ms) * 10_000.0).round() / 10_000.0
        } else {
            0.0
        },
        top,
        rest: Unnamed {
            statements: beyond.len(),
            usd: cents(beyond.iter().map(|s| s.usd).sum()),
        },
        below_one_cent: Unnamed {
            statements: small.len(),
            usd: cents(small.iter().map(|s| s.usd).sum()),
        },
    }
}

/// What a roll did.
#[derive(Debug, Clone, Default, Serialize)]
pub struct RollReport {
    pub periods: Vec<String>,
    pub attempts: usize,
    pub billed_attempts: usize,
    pub rates: usize,
    pub scalars: Scalars,
    /// The current period's spend by statement, per dataset (#1330).
    pub statements: Vec<DatasetStatements>,
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
    statements: &impl StatementSource,
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
    // The current period's spend by statement, per dataset (#1330). A dataset
    // whose lake cannot be read is left out of this section, never priced as
    // zero, and the alarm scalars above are already out.
    let period = report.periods.last().cloned().unwrap_or_default();
    let mut datasets: BTreeMap<(String, String), Vec<&Costed>> = BTreeMap::new();
    for c in current {
        if let Some(d) = &c.attempt.dataset_id {
            datasets
                .entry((c.attempt.org_id.clone(), d.clone()))
                .or_default()
                .push(c);
        }
    }
    for ((org, dataset), costs) in datasets {
        let runs: Vec<String> = costs
            .iter()
            .map(|c| c.attempt.job_id.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        match statements.durations(&org, &dataset, &runs).await {
            Ok(times) => report
                .statements
                .push(statement_costs(&period, &org, &dataset, &costs, &times)),
            Err(e) => tracing::warn!(error = %e, "statement times unavailable; dataset omitted"),
        }
    }
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

    fn costed(job: &str, task: &str, run_seconds: f64, billed: f64) -> Costed {
        let mut a = attempt("t", "o", "i-0abc", "M", "succeeded", 0.0, run_seconds);
        a.job_id = job.into();
        a.task_id = task.into();
        Costed {
            attempt: a,
            allocated_usd: 99.0,
            billed_usd: Some(billed),
        }
    }

    fn time(run: &str, stage: &str, statement: &str, ms: i64) -> StatementTime {
        StatementTime {
            run_id: run.into(),
            stage: stage.into(),
            statement: statement.into(),
            total_ms: ms,
            executions: 1,
        }
    }

    #[test]
    fn a_stages_spend_is_shared_by_statement_time_and_coverage_is_honest() {
        // Two stages of one run: `match` 100 s for $10, `ingest` 50 s for $2.
        let costs = [
            costed("r1", "match", 100.0, 10.0),
            costed("r1", "ingest", 50.0, 2.0),
        ];
        let refs: Vec<&Costed> = costs.iter().collect();
        let times = [
            time("r1", "match", "match_score.create.batch", 60_000),
            time("r1", "match", "pair_candidate.read.block", 20_000),
            // ingest has statements for only 10 of its 50 s.
            time("r1", "ingest", "raw_record.create.append", 10_000),
        ];
        let report = statement_costs("2026-11", "o", "d", &refs, &times);
        assert_eq!(report.spend_usd, 12.0);
        // match: $10 x 60/100 and $10 x 20/100; ingest: $2 x 10/50.
        assert_eq!(report.top[0].statement, "match_score.create.batch");
        assert_eq!(report.top[0].usd, 6.0);
        assert_eq!(report.top[1].usd, 2.0);
        assert_eq!(report.top[2].usd, 0.4);
        assert_eq!(report.attributed_usd, 8.4);
        // (80 + 10) of 150 s explained.
        assert_eq!(report.coverage, 0.6);
        assert_eq!(report.top[0].share, 0.5);
    }

    #[test]
    fn nothing_under_a_cent_is_priced_alone_and_skew_never_overbills() {
        let costs = [costed("r1", "match", 10.0, 0.05)];
        let refs: Vec<&Costed> = costs.iter().collect();
        let mut times = vec![time("r1", "match", "big", 15_000)];
        for i in 0..30 {
            times.push(time("r1", "match", &format!("tiny{i:02}"), 100));
        }
        let report = statement_costs("2026-11", "o", "d", &refs, &times);
        // 18 s of statements in a 10 s attempt: shared over 18 s, never over 10.
        assert!(report.attributed_usd <= 0.05 + 1e-9, "{report:?}");
        assert_eq!(report.coverage, 1.0);
        assert!(
            report.top.iter().all(|s| s.usd >= MIN_QUOTED_USD),
            "{report:?}"
        );
        assert_eq!(report.top.len(), 1);
        assert_eq!(report.below_one_cent.statements, 30);
        // An attempt with no statements at all is spend with zero coverage.
        let bare = [costed("r2", "assemble", 30.0, 1.0)];
        let bare_refs: Vec<&Costed> = bare.iter().collect();
        let report = statement_costs("2026-11", "o", "d", &bare_refs, &[]);
        assert_eq!(
            (report.coverage, report.attributed_usd, report.top.len()),
            (0.0, 0.0, 0)
        );
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
