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

# --- #877: the three standalone alarms that need no cost model -------------

# Attempt failure rate: failed / (failed + done) over an hour, summed across
# classes. Ten is the alarm's metric-query ceiling, which is why no_op and
# skipped (neither ran a failing body) are left out of the denominator. The
# minimum-volume guard stops a single failed attempt in a quiet hour paging.
resource "aws_cloudwatch_metric_alarm" "attempt_failure_rate" {
  alarm_name          = "er-${var.environment}-attempt-failure-rate"
  alarm_description   = "More than ${var.failure_rate_threshold * 100}% of finished task attempts failed in the last hour (with at least ${var.failure_rate_min_attempts} attempts). Read the failing tasks' error_class: a config or precondition burst is a tenant's input, an internal or infrastructure burst is ours."
  comparison_operator = "GreaterThanThreshold"
  threshold           = var.failure_rate_threshold
  evaluation_periods  = 1
  treat_missing_data  = "notBreaching"
  dynamic "metric_query" {
    for_each = merge(
      { for n, class in local.classes : "f${n}" => { class = class, outcome = "failed" } },
      { for n, class in local.classes : "d${n}" => { class = class, outcome = "done" } },
    )
    content {
      id = metric_query.key
      metric {
        namespace   = var.metric_namespace
        metric_name = "er.tasks.completed"
        dimensions  = { resource_class = metric_query.value.class, outcome = metric_query.value.outcome, OTelLib = "er-obs" }
        period      = 3600
        stat        = "Sum"
      }
    }
  }
  metric_query {
    id          = "rate"
    expression  = "IF((FILL(f0,0)+FILL(f1,0)+FILL(f2,0)+FILL(f3,0)+FILL(d0,0)+FILL(d1,0)+FILL(d2,0)+FILL(d3,0)) >= ${var.failure_rate_min_attempts}, (FILL(f0,0)+FILL(f1,0)+FILL(f2,0)+FILL(f3,0)) / (FILL(f0,0)+FILL(f1,0)+FILL(f2,0)+FILL(f3,0)+FILL(d0,0)+FILL(d1,0)+FILL(d2,0)+FILL(d3,0)), 0)"
    return_data = true
  }
  alarm_actions = [var.alerts_topic_arn]
  ok_actions    = [var.alerts_topic_arn]
}

# Spot interruptions: the account's EC2 Spot interruption warnings, counted by
# the rule's own MatchedEvents. Every spot instance here is a Karpenter runner
# node, so this is how "spot is cheap enough to tolerate" gets checked rather
# than trusted. The rule needs no target; matching is what is measured.
resource "aws_cloudwatch_event_rule" "spot_interruption" {
  name        = "er-${var.environment}-spot-interruption"
  description = "EC2 Spot interruption warnings, counted for the spot-interruption alarm."
  event_pattern = jsonencode({
    source        = ["aws.ec2"]
    "detail-type" = ["EC2 Spot Instance Interruption Warning"]
  })
}

resource "aws_cloudwatch_metric_alarm" "spot_interruptions" {
  alarm_name          = "er-${var.environment}-spot-interruptions"
  alarm_description   = "More than ${var.spot_interruptions_threshold} EC2 Spot interruption warnings in an hour. Runner tasks survive one (do-not-disrupt pods are reclaimed under, then relaunched), but a sustained rate means the spot pools are a poor fit: widen the NodePools' instance families or move a class to on-demand."
  namespace           = "AWS/Events"
  metric_name         = "MatchedEvents"
  dimensions          = { RuleName = aws_cloudwatch_event_rule.spot_interruption.name }
  statistic           = "Sum"
  period              = 3600
  comparison_operator = "GreaterThanThreshold"
  threshold           = var.spot_interruptions_threshold
  evaluation_periods  = 1
  treat_missing_data  = "notBreaching"
  alarm_actions       = [var.alerts_topic_arn]
  ok_actions          = [var.alerts_topic_arn]
}

# Impersonation volume: the BFF's audit writer prints one structured line per
# impersonation event, carrying no user or tenant identity (that stays in
# erweb.audit). The metric is a bare count, so a tenant never becomes a
# dimension.
resource "aws_cloudwatch_log_metric_filter" "impersonation" {
  count          = var.containers_log_group_name == null ? 0 : 1
  name           = "er-${var.environment}-impersonation"
  log_group_name = var.containers_log_group_name
  pattern        = "\"er.audit.impersonation\""
  metric_transformation {
    name          = "er.audit.impersonations"
    namespace     = var.metric_namespace
    value         = "1"
    default_value = "0"
  }
}

resource "aws_cloudwatch_metric_alarm" "impersonation_volume" {
  count               = var.containers_log_group_name == null ? 0 : 1
  alarm_name          = "er-${var.environment}-impersonation-volume"
  alarm_description   = "More than ${var.impersonation_threshold} impersonation events (start, stop or impersonated mutation) in an hour. Read erweb.audit for who and which org; operators impersonate to support a tenant, so a burst is either an incident being worked or a compromised operator session."
  namespace           = var.metric_namespace
  metric_name         = "er.audit.impersonations"
  statistic           = "Sum"
  period              = 3600
  comparison_operator = "GreaterThanThreshold"
  threshold           = var.impersonation_threshold
  evaluation_periods  = 1
  treat_missing_data  = "notBreaching"
  alarm_actions       = [var.alerts_topic_arn]
  ok_actions          = [var.alerts_topic_arn]
}
