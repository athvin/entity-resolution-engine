mock_provider "aws" {}

variables {
  environment               = "test"
  region                    = "us-east-2"
  metric_namespace          = "ER/Test"
  alerts_topic_arn          = "arn:aws:sns:us-east-2:123456789012:alerts"
  containers_log_group_name = "/er/test/containers"
}

run "operational_signal_contract" {
  command = plan

  assert {
    condition = (
      aws_cloudwatch_metric_alarm.queue_depth.metric_name == "er.jobs.queue.total" &&
      aws_cloudwatch_metric_alarm.queue_depth.statistic == "Minimum" &&
      aws_cloudwatch_metric_alarm.queue_depth.period == 300 &&
      aws_cloudwatch_metric_alarm.queue_depth.datapoints_to_alarm == 3
    )
    error_message = "Backlog must use the simultaneous total and three sustained periods, never sum independent class peaks."
  }
  assert {
    condition = (
      aws_cloudwatch_metric_alarm.snapshot_missing.treat_missing_data == "breaching" &&
      aws_cloudwatch_metric_alarm.snapshot_missing.metric_name == "er.ops.snapshots" &&
      aws_cloudwatch_metric_alarm.snapshot_missing.dimensions["OTelLib"] == "er-obs" &&
      length(aws_cloudwatch_metric_alarm.snapshot_missing.dimensions) == 1
    )
    error_message = "An absent snapshot must page; otherwise an idle queue and a dead producer look identical."
  }
  assert {
    condition = alltrue([
      for alarm in aws_cloudwatch_metric_alarm.pod_failure :
      alarm.alarm_actions == toset([var.alerts_topic_arn]) &&
      alarm.threshold == 0 && alarm.treat_missing_data == "notBreaching" &&
      length(alarm.metric_query) == 5
    ])
    error_message = "Both pod failure alarms must cover all classes, ignore genuinely quiet periods, and notify the existing topic."
  }
  assert {
    condition = (
      aws_cloudwatch_metric_alarm.age["queue"].threshold == 900 &&
      aws_cloudwatch_metric_alarm.age["queue"].metric_name == "er.jobs.queue.oldest_age" &&
      aws_cloudwatch_metric_alarm.age["pending"].threshold == 600 &&
      aws_cloudwatch_metric_alarm.age["pending"].metric_name == "er.pods.pending.oldest_age" &&
      alltrue([for alarm in aws_cloudwatch_metric_alarm.age :
        alarm.comparison_operator == "GreaterThanThreshold" && alarm.statistic == "Maximum" &&
        alarm.period == 60 && alarm.evaluation_periods == 1 && alarm.datapoints_to_alarm == 1 &&
        alarm.treat_missing_data == "notBreaching" &&
        alarm.dimensions == tomap({ OTelLib = "er-obs" }) &&
        alarm.alarm_actions == toset([var.alerts_topic_arn]) && alarm.ok_actions == toset([var.alerts_topic_arn])
      ])
    )
    error_message = "Age alerts must compare current second-valued maxima against the required bounds and notify the existing topic."
  }
  assert {
    condition = (
      aws_cloudwatch_metric_alarm.pod_snapshot_missing.metric_name == "er.pods.snapshots" &&
      aws_cloudwatch_metric_alarm.pod_snapshot_missing.dimensions == tomap({ OTelLib = "er-obs" }) &&
      aws_cloudwatch_metric_alarm.pod_snapshot_missing.threshold == 1 &&
      aws_cloudwatch_metric_alarm.pod_snapshot_missing.statistic == "Sum" &&
      aws_cloudwatch_metric_alarm.pod_snapshot_missing.period == 60 &&
      aws_cloudwatch_metric_alarm.pod_snapshot_missing.evaluation_periods == 3 &&
      aws_cloudwatch_metric_alarm.pod_snapshot_missing.datapoints_to_alarm == 3 &&
      aws_cloudwatch_metric_alarm.pod_snapshot_missing.treat_missing_data == "breaching" &&
      aws_cloudwatch_metric_alarm.pod_snapshot_missing.alarm_actions == toset([var.alerts_topic_arn])
    )
    error_message = "A failed pod observation must page independently of healthy database snapshots."
  }
  assert {
    condition = alltrue(flatten([
      for widget in jsondecode(aws_cloudwatch_dashboard.operations.dashboard_body).widgets : [
        for row in try(widget.properties.metrics, []) :
        can(row[0].expression) ? true : (
          contains(row, "OTelLib") && contains(row, "er-obs") &&
          contains(["er.http.requests", "er.http.duration", "er.jobs.queue.depth", "er.tasks.active", "er.jobs.dispatch.duration", "er.ops.snapshots", "er.tasks.duration", "er.tasks.retries", "er.tasks.completed", "er.pods.failures", "er.jobs.queue.oldest_age", "er.pods.pending.oldest_age", "er.pods.snapshots"], row[1])
        )
      ]
    ]))
    error_message = "Every metric widget must query a declared instrument with awsemf's actual OTelLib dimension."
  }
}

run "standalone_alarms_877" {
  command = plan

  assert {
    condition = (
      length(aws_cloudwatch_metric_alarm.attempt_failure_rate.metric_query) == 9 &&
      aws_cloudwatch_metric_alarm.attempt_failure_rate.threshold == 0.25 &&
      aws_cloudwatch_metric_alarm.attempt_failure_rate.treat_missing_data == "notBreaching" &&
      alltrue([for q in aws_cloudwatch_metric_alarm.attempt_failure_rate.metric_query :
        q.metric[0].period == 3600 if length(q.metric) > 0
      ])
    )
    error_message = "The failure-rate alarm must read failed and done for every class over an hour, within the ten-query ceiling, behind a volume guard."
  }
  assert {
    condition = (
      aws_cloudwatch_metric_alarm.spot_interruptions.namespace == "AWS/Events" &&
      aws_cloudwatch_metric_alarm.spot_interruptions.metric_name == "MatchedEvents" &&
      aws_cloudwatch_metric_alarm.spot_interruptions.dimensions["RuleName"] == "er-test-spot-interruption" &&
      jsondecode(aws_cloudwatch_event_rule.spot_interruption.event_pattern)["detail-type"] == ["EC2 Spot Instance Interruption Warning"]
    )
    error_message = "Spot interruptions are counted from the interruption-warning rule's own matches."
  }
  assert {
    condition = (
      aws_cloudwatch_log_metric_filter.impersonation[0].log_group_name == "/er/test/containers" &&
      aws_cloudwatch_log_metric_filter.impersonation[0].metric_transformation[0].name == "er.audit.impersonations" &&
      aws_cloudwatch_metric_alarm.impersonation_volume[0].dimensions == null &&
      aws_cloudwatch_metric_alarm.impersonation_volume[0].alarm_actions == toset([var.alerts_topic_arn])
    )
    error_message = "Impersonation volume is a dimensionless count from the container log group, alarmed to the existing topic."
  }
}

run "no_log_group_no_impersonation_alarm" {
  command = plan
  variables {
    containers_log_group_name = null
  }
  assert {
    condition     = length(aws_cloudwatch_metric_alarm.impersonation_volume) == 0 && length(aws_cloudwatch_log_metric_filter.impersonation) == 0
    error_message = "Without a log group there is nothing to count, so no alarm may claim coverage."
  }
}

