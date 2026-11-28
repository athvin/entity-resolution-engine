//! The AWS halves of the cost roll-up (#667): instance costs from the CUR
//! through Athena, and the alarm scalars to CloudWatch.
//!
//! Athena runs in the `er-cost` workgroup, whose 10 GiB per-query scan cap
//! means a query that forgets its partition filter fails rather than bills.
//! Every query filters on `billing_period` (the projected partition), and
//! every instance id is checked before it is quoted.

use std::collections::BTreeMap;
use std::time::Duration;

use chrono::{DateTime, Utc};

use crate::cost_roll::{CurSource, MetricSink, Scalars, valid_instance_id};

/// The metric namespace the alarms read.
pub const NAMESPACE: &str = "ER/Cost";
/// Ids per query: keeps the SQL well under Athena's statement size limit.
const CHUNK: usize = 400;

/// The CUR through Athena.
pub struct Athena {
    pub client: aws_sdk_athena::Client,
    pub workgroup: String,
    pub table: String,
}

impl Athena {
    /// From the environment: `ER_COST_ATHENA_WORKGROUP` (default `er-cost`)
    /// and `ER_COST_CUR_TABLE` (default `er_cur.cur2`).
    pub async fn from_env() -> Self {
        let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
        Self {
            client: aws_sdk_athena::Client::new(&config),
            workgroup: std::env::var("ER_COST_ATHENA_WORKGROUP")
                .unwrap_or_else(|_| "er-cost".into()),
            table: std::env::var("ER_COST_CUR_TABLE").unwrap_or_else(|_| "er_cur.cur2".into()),
        }
    }

    /// Run one query to completion and return its rows (header excluded).
    async fn query(&self, sql: &str) -> Result<Vec<Vec<Option<String>>>, String> {
        let started = self
            .client
            .start_query_execution()
            .work_group(&self.workgroup)
            .query_string(sql)
            .send()
            .await
            .map_err(|e| {
                format!(
                    "athena start: {}",
                    aws_sdk_athena::error::DisplayErrorContext(e)
                )
            })?;
        let id = started.query_execution_id().unwrap_or_default().to_string();
        for _ in 0..120 {
            let got = self
                .client
                .get_query_execution()
                .query_execution_id(&id)
                .send()
                .await
                .map_err(|e| {
                    format!(
                        "athena status: {}",
                        aws_sdk_athena::error::DisplayErrorContext(e)
                    )
                })?;
            let state = got
                .query_execution()
                .and_then(|q| q.status())
                .and_then(|s| s.state())
                .map(|s| s.as_str().to_string())
                .unwrap_or_default();
            match state.as_str() {
                "SUCCEEDED" => break,
                "FAILED" | "CANCELLED" => {
                    let why = got
                        .query_execution()
                        .and_then(|q| q.status())
                        .and_then(|s| s.state_change_reason())
                        .unwrap_or_default()
                        .to_string();
                    return Err(format!("athena query {state}: {why}"));
                }
                _ => tokio::time::sleep(Duration::from_secs(2)).await,
            }
        }
        let mut rows = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let page = self
                .client
                .get_query_results()
                .query_execution_id(&id)
                .set_next_token(token.clone())
                .send()
                .await
                .map_err(|e| {
                    format!(
                        "athena results: {}",
                        aws_sdk_athena::error::DisplayErrorContext(e)
                    )
                })?;
            if let Some(set) = page.result_set() {
                for row in set.rows() {
                    rows.push(
                        row.data()
                            .iter()
                            .map(|d| d.var_char_value().map(str::to_string))
                            .collect(),
                    );
                }
            }
            token = page.next_token().map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        // The first row of a SELECT result is the header.
        if !rows.is_empty() {
            rows.remove(0);
        }
        Ok(rows)
    }
}

fn valid_period(p: &str) -> bool {
    p.len() == 7
        && p.as_bytes()[4] == b'-'
        && p.bytes()
            .enumerate()
            .all(|(i, b)| i == 4 || b.is_ascii_digit())
}

impl CurSource for Athena {
    async fn instance_costs(
        &self,
        period: &str,
        instance_ids: &[String],
    ) -> Result<BTreeMap<String, f64>, String> {
        if !valid_period(period) {
            return Err(format!("not a billing period: {period:?}"));
        }
        let ids: Vec<&String> = instance_ids
            .iter()
            .filter(|i| valid_instance_id(i))
            .collect();
        let mut out = BTreeMap::new();
        for chunk in ids.chunks(CHUNK) {
            let list = chunk
                .iter()
                .map(|i| format!("'{i}'"))
                .collect::<Vec<_>>()
                .join(",");
            // Usage, spot and the Auto Mode fee all key on the instance id.
            let sql = format!(
                "SELECT line_item_resource_id, sum(line_item_unblended_cost) FROM {} \
                 WHERE billing_period = '{period}' AND line_item_resource_id IN ({list}) GROUP BY 1",
                self.table
            );
            for row in self.query(&sql).await? {
                if let (Some(Some(id)), Some(Some(usd))) = (row.first(), row.get(1)) {
                    out.insert(id.clone(), usd.parse::<f64>().unwrap_or(0.0));
                }
            }
        }
        Ok(out)
    }

    async fn latest_usage_end(&self) -> Result<Option<DateTime<Utc>>, String> {
        let (period, _, _) = crate::cost_roll::period_bounds(Utc::now());
        let sql = format!(
            "SELECT max(line_item_usage_end_date) FROM {} WHERE billing_period = '{period}'",
            self.table
        );
        let rows = self.query(&sql).await?;
        Ok(rows
            .first()
            .and_then(|r| r.first())
            .and_then(Clone::clone)
            // Athena renders a timestamp as `YYYY-MM-DD HH:MM:SS.fff`.
            .and_then(|s| chrono::NaiveDateTime::parse_from_str(&s, "%Y-%m-%d %H:%M:%S%.f").ok())
            .map(|n| n.and_utc()))
    }
}

/// The alarm scalars to CloudWatch, with no tenant dimension.
pub struct CloudWatch {
    pub client: aws_sdk_cloudwatch::Client,
    /// `ER_ENVIRONMENT` (dev/prod): the one dimension, so environments do not mix.
    pub environment: String,
}

impl CloudWatch {
    pub async fn from_env() -> Self {
        let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
        Self {
            client: aws_sdk_cloudwatch::Client::new(&config),
            environment: std::env::var("ER_ENVIRONMENT").unwrap_or_else(|_| "dev".into()),
        }
    }
}

impl MetricSink for CloudWatch {
    async fn put(&self, s: &Scalars) -> Result<(), String> {
        use aws_sdk_cloudwatch::types::{Dimension, MetricDatum, StandardUnit};
        let dim = Dimension::builder()
            .name("Environment")
            .value(&self.environment)
            .build();
        let datum = |name: &str, value: f64, unit: StandardUnit| {
            MetricDatum::builder()
                .metric_name(name)
                .value(if value.is_finite() { value } else { 1.0e6 })
                .unit(unit)
                .dimensions(dim.clone())
                .build()
        };
        self.client
            .put_metric_data()
            .namespace(NAMESPACE)
            .metric_data(datum(
                "MaxTenantSpendShare",
                s.max_tenant_spend_share,
                StandardUnit::None,
            ))
            .metric_data(datum(
                "MaxTenantDisproportion",
                s.max_tenant_disproportion,
                StandardUnit::None,
            ))
            .metric_data(datum(
                "FailedRunSpendShare",
                s.failed_run_spend_share,
                StandardUnit::None,
            ))
            .metric_data(datum(
                "MaxCostPerMillionMoMChange",
                s.max_cost_per_million_mom_change,
                StandardUnit::None,
            ))
            .metric_data(datum(
                "CurStalenessHours",
                s.cur_staleness_hours,
                StandardUnit::None,
            ))
            .send()
            .await
            .map_err(|e| {
                format!(
                    "cloudwatch put: {}",
                    aws_sdk_cloudwatch::error::DisplayErrorContext(e)
                )
            })?;
        Ok(())
    }
}
