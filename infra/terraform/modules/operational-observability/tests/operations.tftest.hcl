mock_provider "aws" {}

variables {
  environment      = "test"
  region           = "us-east-2"
  metric_namespace = "ER/Test"
  alerts_topic_arn = "arn:aws:sns:us-east-2:123456789012:alerts"
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
    condition = alltrue(flatten([
      for widget in jsondecode(aws_cloudwatch_dashboard.operations.dashboard_body).widgets : [
        for row in try(widget.properties.metrics, []) :
        can(row[0].expression) ? true : (
          contains(row, "OTelLib") && contains(row, "er-obs") &&
          contains(["er.http.requests", "er.http.duration", "er.jobs.queue.depth", "er.tasks.active", "er.jobs.dispatch.duration", "er.ops.snapshots", "er.tasks.duration", "er.tasks.retries", "er.tasks.completed", "er.pods.failures"], row[1])
        )
      ]
    ]))
    error_message = "Every metric widget must query a declared instrument with awsemf's actual OTelLib dimension."
  }
}
