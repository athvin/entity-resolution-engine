# Operational metrics have closed dimensions in er-obs/src/registry.rs. OTelLib
# is injected by awsemf even with NoDimensionRollup; every query includes it.
locals {
  age_signals = {
    queue   = { metric = "er.jobs.queue.oldest_age", threshold = 900, name = "queue-oldest", title = "Oldest queued job (seconds, includes backoff)", action = "Inspect dispatcher readiness, job backoff, concurrency and runner capacity." }
    pending = { metric = "er.pods.pending.oldest_age", threshold = 600, name = "pod-pending", title = "Oldest Pending runner pod (seconds)", action = "Inspect pod scheduling, image pulls, node capacity and account vCPU quotas." }
  }
  age_widgets = [for index, key in ["queue", "pending"] : {
    type = "metric", x = index * 12, y = 46, width = 12, height = 6
    properties = {
      title   = local.age_signals[key].title, region = var.region, view = "timeSeries", period = 60, stat = "Maximum"
      metrics = [[var.metric_namespace, local.age_signals[key].metric, "OTelLib", "er-obs"]]
    }
  }]

  classes = ["S", "M", "L", "other"]
  stages  = ["init", "ingest", "standardize", "match", "reconcile", "assemble", "finish", "apply_staged", "write_back_scan", "reconcile_schema", "profile", "maintain", "extract", "train", "lake_init", "other"]
  panels = [
    { title = "API responses / minute", name = "er.http.requests", dimension = "status_class", values = ["1xx", "2xx", "3xx", "4xx", "5xx", "other"], stat = "Sum" },
    { title = "API response time (mean ms)", name = "er.http.duration", dimension = "status_class", values = ["2xx", "4xx", "5xx"], stat = "Average" },
    { title = "Queued jobs (includes backoff)", name = "er.jobs.queue.depth", dimension = "resource_class", values = local.classes, stat = "Maximum" },
    { title = "Live pod-task claims (includes launch/cancel wait)", name = "er.tasks.active", dimension = "resource_class", values = local.classes, stat = "Maximum" },
    { title = "Enqueue to claim (mean ms)", name = "er.jobs.dispatch.duration", dimension = "resource_class", values = local.classes, stat = "Average" },
  ]
  base_widgets = [for index, panel in local.panels : {
    type = "metric", x = (index % 2) * 12, y = floor(index / 2) * 6 + 4, width = 12, height = 6
    properties = {
      title   = panel.title, region = var.region, view = "timeSeries", period = 60, stat = panel.stat
      metrics = [for value in panel.values : [var.metric_namespace, panel.name, panel.dimension, value, "OTelLib", "er-obs", { label = value }]]
    }
  }]
  stage_widgets = [for index, class in local.classes : {
    type = "metric", x = (index % 2) * 12, y = floor(index / 2) * 6 + 22, width = 12, height = 6
    properties = {
      title   = "Task duration — ${class} (mean ms)", region = var.region, view = "timeSeries", period = 60, stat = "Average"
      metrics = [for stage in local.stages : [var.metric_namespace, "er.tasks.duration", "resource_class", class, "stage", stage, "OTelLib", "er-obs", { label = stage }]]
    }
  }]
  rate_widgets = [for index, signal in ["retry", "no_op"] : {
    type = "metric", x = index * 12, y = 34, width = 12, height = 6
    properties = {
      title = "${signal} task events / second", region = var.region, view = "timeSeries", period = 60, stat = "Sum"
      metrics = concat(
        [for n, class in local.classes : concat(
          [var.metric_namespace, signal == "retry" ? "er.tasks.retries" : "er.tasks.completed", "resource_class", class],
          signal == "no_op" ? ["outcome", "no_op"] : [],
          ["OTelLib", "er-obs", { id = "m${n}", visible = false }]
        )],
        [for n, class in local.classes : [{ expression = "m${n}/PERIOD(m${n})", label = class, id = "r${n}" }]]
      )
    }
  }]
}

resource "aws_cloudwatch_dashboard" "operations" {
  dashboard_name = "er-${var.environment}-operations"
  dashboard_body = jsonencode({ widgets = concat(
    [{ type = "text", x = 0, y = 0, width = 24, height = 4, properties = { markdown = "# ER ${var.environment} operations (shared deployment)\nAPI errors are the 5xx response series. Queue and concurrency are current gauges; the database and managed-pod snapshot heartbeats must be present before an empty age chart means idle. A missing or zero pod heartbeat means Kubernetes observations are unavailable even if database snapshots continue. Counters are interval deltas, so rates divide Sum by the period. Duration charts use means, not percentiles.\n\nPer-tenant data quality remains in the authenticated dataset dashboard (`/{org}/{dataset}/dashboard`) and dataset `/metrics` API, backed by durable lake data. Tenant and dataset identifiers never become CloudWatch dimensions. Task counters are operational observations, not an accounting ledger." } }],
    local.base_widgets,
    [{ type = "metric", x = 12, y = 16, width = 12, height = 6, properties = {
      title   = "Successful queue snapshots / minute (expected about 2)", region = var.region, view = "timeSeries", period = 60, stat = "Sum"
      metrics = [[var.metric_namespace, "er.ops.snapshots", "OTelLib", "er-obs"]]
    } }],
    local.stage_widgets,
    local.rate_widgets,
    [{ type = "metric", x = 0, y = 40, width = 24, height = 6, properties = {
      title = "Confirmed pod OOM kills and evictions / minute", region = var.region, view = "timeSeries", period = 60, stat = "Sum"
      metrics = flatten([for class in local.classes : [for reason in ["oom", "evicted"] : {
        row = [var.metric_namespace, "er.pods.failures", "resource_class", class, "reason", reason, "OTelLib", "er-obs", { label = "${class} ${reason}" }]
      }]])[*].row
    } }],
    local.age_widgets,
    [{ type = "metric", x = 0, y = 52, width = 24, height = 6, properties = {
      title   = "Successful managed-pod snapshots / minute (expected about 2)", region = var.region, view = "timeSeries", period = 60, stat = "Sum"
      metrics = [[var.metric_namespace, "er.pods.snapshots", "OTelLib", "er-obs"]]
    } }]
  ) })
}

resource "aws_cloudwatch_metric_alarm" "queue_depth" {
  alarm_name          = "er-${var.environment}-queue-backlog"
  alarm_description   = "At least ${var.queue_threshold} jobs have waited for 15 minutes. Inspect dispatcher readiness, per-class concurrency, pod capacity and job backoff. Includes scheduled delays; confirm service rate before changing capacity."
  comparison_operator = "GreaterThanOrEqualToThreshold"
  threshold           = var.queue_threshold
  evaluation_periods  = 3
  datapoints_to_alarm = 3
  treat_missing_data  = "notBreaching"
  namespace           = var.metric_namespace
  metric_name         = "er.jobs.queue.total"
  dimensions          = { OTelLib = "er-obs" }
  period              = 300
  statistic           = "Minimum"
  alarm_actions       = [var.alerts_topic_arn]
  ok_actions          = [var.alerts_topic_arn]
}

resource "aws_cloudwatch_metric_alarm" "pod_failure" {
  for_each            = toset(["oom", "evicted"])
  alarm_name          = "er-${var.environment}-pod-${each.key}"
  alarm_description   = "A runner ${each.key} was confirmed in the last five minutes. Inspect the task's durable termination reason and resource class; check memory, ephemeral storage and node disruption before retrying."
  comparison_operator = "GreaterThanThreshold"
  threshold           = 0
  evaluation_periods  = 1
  treat_missing_data  = "notBreaching"
  dynamic "metric_query" {
    for_each = { for n, class in local.classes : "p${n}" => class }
    content {
      id = metric_query.key
      metric {
        namespace   = var.metric_namespace
        metric_name = "er.pods.failures"
        dimensions  = { resource_class = metric_query.value, reason = each.key, OTelLib = "er-obs" }
        period      = 300
        stat        = "Sum"
      }
    }
  }
  metric_query {
    id          = "total"
    expression  = "SUM([FILL(p0,0),FILL(p1,0),FILL(p2,0),FILL(p3,0)])"
    return_data = true
  }
  alarm_actions = [var.alerts_topic_arn]
  ok_actions    = [var.alerts_topic_arn]
}

resource "aws_cloudwatch_metric_alarm" "snapshot_missing" {
  alarm_name          = "er-${var.environment}-operational-snapshot-missing"
  alarm_description   = "No successful queue/concurrency snapshots for three minutes. Check dispatcher process, database reads and the metric export pipeline; empty queue charts are not evidence of idle until this recovers."
  namespace           = var.metric_namespace
  metric_name         = "er.ops.snapshots"
  dimensions          = { OTelLib = "er-obs" }
  statistic           = "Sum"
  period              = 60
  comparison_operator = "LessThanThreshold"
  threshold           = 1
  evaluation_periods  = 3
  datapoints_to_alarm = 3
  treat_missing_data  = "breaching"
  alarm_actions       = [var.alerts_topic_arn]
  ok_actions          = [var.alerts_topic_arn]
}


resource "aws_cloudwatch_metric_alarm" "age" {
  for_each            = local.age_signals
  alarm_name          = "er-${var.environment}-${each.value.name}"
  alarm_description   = "${each.value.title} exceeds ${each.value.threshold} seconds. ${each.value.action} Require the matching snapshot heartbeat before treating a quiet chart as healthy."
  namespace           = var.metric_namespace
  metric_name         = each.value.metric
  dimensions          = { OTelLib = "er-obs" }
  statistic           = "Maximum"
  period              = 60
  comparison_operator = "GreaterThanThreshold"
  threshold           = each.value.threshold
  evaluation_periods  = 1
  datapoints_to_alarm = 1
  treat_missing_data  = "notBreaching"
  alarm_actions       = [var.alerts_topic_arn]
  ok_actions          = [var.alerts_topic_arn]
}

resource "aws_cloudwatch_metric_alarm" "pod_snapshot_missing" {
  alarm_name          = "er-${var.environment}-pod-snapshot-missing"
  alarm_description   = "No successful managed-pod snapshots for three minutes. Inspect Kubernetes API access, dispatcher readiness and metric export; the pending-age gauge is stale until this recovers. Database snapshots are independent."
  namespace           = var.metric_namespace
  metric_name         = "er.pods.snapshots"
  dimensions          = { OTelLib = "er-obs" }
  statistic           = "Sum"
  period              = 60
  comparison_operator = "LessThanThreshold"
  threshold           = 1
  evaluation_periods  = 3
  datapoints_to_alarm = 3
  treat_missing_data  = "breaching"
  alarm_actions       = [var.alerts_topic_arn]
  ok_actions          = [var.alerts_topic_arn]
}
