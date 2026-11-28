# Cost attribution in prod (#1331), the twin of envs/dev/cost-roll.tf (#667):
# the `er-cost-roll` CronJob — `er-server cost-roll` on the prod er-api
# ServiceAccount — reads the CUR through Athena and publishes five aggregate
# scalars with Environment=prod, which the alarms below watch. The CUR, the
# `er-cost` workgroup and its results bucket are account-wide (core/), so the
# policy is the same; only the role and the alarm names and dimension differ.
#
# The policy is read-only on billing data and write-only to one metric
# namespace:
#   - Athena: start/read queries in the `er-cost` workgroup only. That
#     workgroup enforces its own result location and a 10 GiB scan cap, so a
#     query can neither redirect output nor bill a full-table scan.
#   - Glue: read the `er_cur` database and its tables (the schema Athena plans
#     against).
#   - S3: read the CUR export bucket; read and write the workgroup's results
#     prefix (Athena writes results as the caller).
#   - CloudWatch: PutMetricData, conditioned on the `ER/Cost` namespace.
# The buckets are SSE-S3 (core/athena.tf, core/cur.tf), so no KMS grant.

locals {
  cost_account       = data.aws_caller_identity.current.account_id
  cost_cur_bucket    = "er-cur-${local.cost_account}"
  cost_result_bucket = "er-athena-results-${local.cost_account}"
  # The same topic operations.tf hands the operational-observability module.
  cost_alerts_topic_arn = "arn:aws:sns:us-east-2:${local.cost_account}:er-alerts"
}

data "aws_iam_policy_document" "er_api_cost_roll" {
  statement {
    sid       = "AthenaCostWorkgroup"
    actions   = ["athena:StartQueryExecution", "athena:GetQueryExecution", "athena:GetQueryResults", "athena:GetWorkGroup"]
    resources = ["arn:aws:athena:us-east-2:${local.cost_account}:workgroup/er-cost"]
  }
  statement {
    sid     = "GlueCurCatalog"
    actions = ["glue:GetDatabase", "glue:GetTable", "glue:GetTables", "glue:GetPartition", "glue:GetPartitions"]
    resources = [
      "arn:aws:glue:us-east-2:${local.cost_account}:catalog",
      "arn:aws:glue:us-east-2:${local.cost_account}:database/er_cur",
      "arn:aws:glue:us-east-2:${local.cost_account}:table/er_cur/*",
    ]
  }
  statement {
    sid       = "CurRead"
    actions   = ["s3:GetObject", "s3:ListBucket", "s3:GetBucketLocation"]
    resources = ["arn:aws:s3:::${local.cost_cur_bucket}", "arn:aws:s3:::${local.cost_cur_bucket}/*"]
  }
  statement {
    sid     = "AthenaResults"
    actions = ["s3:GetObject", "s3:PutObject", "s3:ListBucket", "s3:GetBucketLocation", "s3:AbortMultipartUpload"]
    resources = [
      "arn:aws:s3:::${local.cost_result_bucket}",
      "arn:aws:s3:::${local.cost_result_bucket}/q/*",
    ]
  }
  statement {
    sid       = "CostMetrics"
    actions   = ["cloudwatch:PutMetricData"]
    resources = ["*"]
    condition {
      test     = "StringEquals"
      variable = "cloudwatch:namespace"
      values   = ["ER/Cost"]
    }
  }
}

resource "aws_iam_role_policy" "er_api_cost_roll" {
  name   = "er-prod-api-cost-roll"
  role   = aws_iam_role.er_api_pod.id
  policy = data.aws_iam_policy_document.er_api_cost_roll.json
}

# --- The five alarms (P17) ----------------------------------------------------
# Aggregate scalars with no tenant dimension; the tenant is in the roll-up's
# report (the CronJob's log), never a label. The roll runs daily, so each
# alarm evaluates one-day periods; missing data is not breaching (a day the
# roll did not run is the CronJob's own failure, visible in `kubectl get jobs`).

locals {
  cost_alarms = {
    "max-tenant-spend-share" = {
      metric      = "MaxTenantSpendShare"
      threshold   = 0.5
      operator    = "GreaterThanThreshold"
      periods     = 1
      datapoints  = 1
      description = "One org is more than half of the period's runner spend. Read the er-cost-roll job's report for which org; a single tenant dominating cost is a pricing or runaway-job signal."
    }
    "max-tenant-disproportion" = {
      metric      = "MaxTenantDisproportion"
      threshold   = 3
      operator    = "GreaterThanOrEqualToThreshold"
      periods     = 3
      datapoints  = 2
      description = "A dataset's share of spend is at least 3x its share of records on 2 of the last 3 days: a small tenant costing like a big one. The report names the (org, dataset)."
    }
    "failed-run-spend-share" = {
      metric      = "FailedRunSpendShare"
      threshold   = 0.2
      operator    = "GreaterThanThreshold"
      periods     = 1
      datapoints  = 1
      description = "More than 20% of runner spend went to attempts that failed, timed out or were retried. Check the failing task class and its error_class."
    }
    "cost-per-million-mom" = {
      metric      = "MaxCostPerMillionMoMChange"
      threshold   = 0.5
      operator    = "GreaterThanThreshold"
      periods     = 2
      datapoints  = 2
      description = "A dataset's cost per million records rose more than 50% month over month on two consecutive rolls. Per dataset, not per org: an org's blend hides the degrading dataset."
    }
    "cur-staleness" = {
      metric      = "CurStalenessHours"
      threshold   = 48
      operator    = "GreaterThanThreshold"
      periods     = 1
      datapoints  = 1
      description = "The CUR's newest line is more than 48h old: billed_usd is going stale and attribution is falling back to allocated. Check the core/cur.tf export."
    }
  }
}

resource "aws_cloudwatch_metric_alarm" "cost" {
  for_each = local.cost_alarms

  alarm_name          = "er-prod-cost-${each.key}"
  namespace           = "ER/Cost"
  metric_name         = each.value.metric
  dimensions          = { Environment = "prod" }
  statistic           = "Maximum"
  period              = 86400
  evaluation_periods  = each.value.periods
  datapoints_to_alarm = each.value.datapoints
  threshold           = each.value.threshold
  comparison_operator = each.value.operator
  treat_missing_data  = "notBreaching"
  alarm_description   = each.value.description
  alarm_actions       = [local.cost_alerts_topic_arn]
  ok_actions          = [local.cost_alerts_topic_arn]
}
