# The alarms that need no new emission.
#
# Every metric here is one CloudWatch publishes on its own, so these work the
# moment they are applied — nothing in the Rust has to ship first. That is the
# whole selection criterion: the alarms that depend on `er-server` emitting
# something wait for P12, and an alarm wired to a metric nobody publishes sits
# in INSUFFICIENT_DATA forever while looking configured.
#
# ## These are runaway detectors, not budgets
#
# The thresholds are deliberately generous. The standing instruction on this
# system is to lean towards over-logging when the call is close, so an alarm
# that fires at a sensible monthly spend would fire constantly and be muted
# within a week — and a muted alarm is worse than none, because the console
# still shows it. Each threshold below is set where the only explanation is a
# loop, a misconfiguration or a runaway, with the arithmetic stated so the next
# person can move it on purpose rather than by feel.
#
# Spend itself is NOT alarmed here. The two Budgets in envs/core already cover
# it, they are forecast-aware (which a CloudWatch threshold is not), and
# `AWS/Billing` `EstimatedCharges` is only published in `us-east-1` — so a spend
# alarm in this stack would need its own cross-region topic to say something
# already being said.

# The topic lives in envs/core. Its ARN is CONSTRUCTED rather than read, and
# that is a correction rather than a shortcut.
#
# The first version used `data "aws_sns_topic"`, matching the
# `aws_iam_openid_connect_provider` pattern beside it on the stated reasoning
# that it "fails loudly if envs/core has not been applied". That pattern works
# there because core was applied long ago. It cannot work for a resource born in
# the SAME change: `plan (core)` and `plan (dev)` run in parallel against the
# account as it is now, so dev's plan asked for a topic that does not exist yet
# and failed with `reading SNS Topic (er-alerts): empty result`. A data source
# can only read the past.
#
# Ordering is still guaranteed, just at apply rather than plan: the apply matrix
# is `max-parallel: 1` with `core` ahead of `dev`, so the topic exists before
# these alarms are created. An SNS ARN is fully determined by account, region
# and name, so constructing it is exact rather than a guess.
locals {
  # `data.aws_caller_identity.current` is declared in bedrock.tf — one per
  # module, so it is reused here rather than re-declared.
  alerts_topic_arn = "arn:aws:sns:us-east-2:${data.aws_caller_identity.current.account_id}:er-alerts"
}

locals {
  # 5 GiB/day. `/er/dev/containers` is STANDARD class today — verified against
  # the live account, not assumed — so the rate is $0.50/GB: ~$2.50/day, or
  # ~$75/month sustained. That is well above anything this system produces at
  # one developer's load, and far enough above it that a breach means a loop
  # rather than a busy day. The container stream carries every `eprintln!` in
  # er-server plus every pipeline stage, so it gets the larger share.
  #
  # P4 proposes moving this group to INFREQUENT_ACCESS, which would halve the
  # rate. Worth doing, but NOT bundled here: the class is fixed at creation, so
  # changing it in place is a destroy-and-recreate that takes the group's
  # existing events with it. That belongs in its own PR whose plan output has
  # been read, not in an unattended apply riding along with an alarm.
  containers_daily_bytes = 5 * 1024 * 1024 * 1024

  # 1 GiB/day. This stream carries the collector's own export of metrics and
  # traces, which is structured and bounded; a gigabyte a day of it is already
  # anomalous.
  metrics_daily_bytes = 1 * 1024 * 1024 * 1024

  # 500 GiB/day scanned. At $5/TB that is ~$2.50/day. The workgroup already
  # caps a SINGLE query at 10 GiB (envs/core/athena.tf), so this cannot be one
  # bad `WHERE` — it is the same query running fifty times, which is what a
  # scheduled roll-up with a broken partition predicate looks like.
  athena_daily_bytes = 500 * 1024 * 1024 * 1024
}

# The observability self-guard, and the reason this file exists at all.
#
# The telemetry programme's own failure mode is that it becomes the largest line
# on the bill. An ingest alarm is the one measurement that catches that, and it
# has to be a CloudWatch-native metric precisely because a telemetry pipeline
# cannot be trusted to report on its own runaway.
resource "aws_cloudwatch_metric_alarm" "containers_log_ingest" {
  alarm_name  = "er-dev-containers-log-ingest"
  namespace   = "AWS/Logs"
  metric_name = "IncomingBytes"
  dimensions = {
    LogGroupName = aws_cloudwatch_log_group.containers.name
  }

  statistic           = "Sum"
  period              = 86400
  evaluation_periods  = 1
  comparison_operator = "GreaterThanThreshold"
  threshold           = local.containers_daily_bytes

  # A day with no deploys and no runs legitimately ships almost nothing, and
  # `missing` would otherwise read as a breach once the period rolls over.
  treat_missing_data = "notBreaching"

  alarm_description = "Container log ingest exceeded 5 GiB in a day (~$2.50/day at the STANDARD rate of $0.50/GB). Expect a crash loop, a retry storm, or a log level left at debug. If this group is ever moved to INFREQUENT_ACCESS, that class supports no metric filters, Live Tail or subscription filters, which would make this alarm the only guard on it."

  alarm_actions = [local.alerts_topic_arn]
  ok_actions    = [local.alerts_topic_arn]
}

resource "aws_cloudwatch_metric_alarm" "metrics_log_ingest" {
  alarm_name  = "er-dev-metrics-log-ingest"
  namespace   = "AWS/Logs"
  metric_name = "IncomingBytes"
  dimensions = {
    LogGroupName = aws_cloudwatch_log_group.metrics.name
  }

  statistic           = "Sum"
  period              = 86400
  evaluation_periods  = 1
  comparison_operator = "GreaterThanThreshold"
  threshold           = local.metrics_daily_bytes

  treat_missing_data = "notBreaching"

  alarm_description = "Metrics/traces log ingest exceeded 1 GiB in a day. This stream is structured and bounded, so a breach most likely means tail sampling stopped dropping anything — check that `tail_sampling` still precedes `batch` in the collector pipeline."

  alarm_actions = [local.alerts_topic_arn]
  ok_actions    = [local.alerts_topic_arn]
}

# Athena is the one service here that bills per byte READ rather than per byte
# stored, so a mistake costs money immediately and leaves nothing behind to show
# what happened.
resource "aws_cloudwatch_metric_alarm" "athena_data_scanned" {
  alarm_name  = "er-dev-athena-data-scanned"
  namespace   = "AWS/Athena"
  metric_name = "DataScannedInBytes"
  dimensions = {
    WorkGroup = "er-cost"
  }

  statistic           = "Sum"
  period              = 86400
  evaluation_periods  = 1
  comparison_operator = "GreaterThanThreshold"
  threshold           = local.athena_daily_bytes

  treat_missing_data = "notBreaching"

  alarm_description = "Athena scanned more than 500 GiB in a day in the er-cost workgroup. A single query cannot do this — the workgroup caps one at 10 GiB — so this is the same query repeating, i.e. a partition predicate that stopped pruning."

  alarm_actions = [local.alerts_topic_arn]
  ok_actions    = [local.alerts_topic_arn]
}
