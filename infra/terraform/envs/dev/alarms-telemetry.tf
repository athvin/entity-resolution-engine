# Alarms on the telemetry pipeline's OWN health — the ones whose metrics have
# to ship in er-obs before they exist.
#
# alarms.tf holds the alarms that need no new emission; everything in this
# file watches a metric this repository publishes itself, so each sits in
# INSUFFICIENT_DATA until the code that emits it is deployed. That is the
# deliberate order: the metric and the alarm arrive in the same change, and a
# quiet first hour reads as insufficient data rather than as a breach.
#
# ## The failure this exists for — #941
#
# On 2026-11-08 an er-api pod exported ZERO spans for its entire 40-minute
# lifetime: the exporter installed, the boot log said so, request spans were
# recorded (they were in stdout), and its metric pipeline kept flowing to the
# same collector endpoint the whole time. Nothing paged, because nothing
# watched span volume per role. It was found by accident, by another ticket's
# live probe, and self-healed on pod restart — meaning it will happen again
# and look identical to a healthy pod until somebody queries for spans that
# are not there.
#
# The alarm is SELF-REFERENCING on purpose: `spans_ended` rises whenever the
# process does anything, `spans_exported` must then rise within seconds. A
# process doing nothing is quiet on both and does not page; a process whose
# exports are wedged diverges, and that divergence is the only shape this
# failure has. `RATE` over `Maximum` reads the cumulative counters the
# PeriodicReader publishes every ~30s.
#
# `task` is deliberately not alarmed: an exec-task pod flushes once at exit,
# so a long run legitimately shows ended>0 and exported==0 for its whole
# lifetime — the same expression would page on every slow job.
resource "aws_cloudwatch_metric_alarm" "span_export_silence" {
  for_each = toset(["api", "dispatcher"])

  alarm_name = "er-dev-span-export-silence-${each.key}"

  comparison_operator = "GreaterThanThreshold"
  threshold           = 0
  evaluation_periods  = 3
  datapoints_to_alarm = 3

  # A quiet process is not a wedged one: no spans ending means nothing to
  # export, and missing data here is the norm outside working hours.
  treat_missing_data = "notBreaching"

  metric_query {
    id = "ended"
    metric {
      namespace   = "ER/Dev"
      metric_name = "er.telemetry.spans_ended"
      dimensions  = { "er.role" = each.key }
      period      = 300
      stat        = "Maximum"
    }
  }

  metric_query {
    id = "exported"
    metric {
      namespace   = "ER/Dev"
      metric_name = "er.telemetry.spans_exported"
      dimensions  = { "er.role" = each.key }
      period      = 300
      stat        = "Maximum"
    }
  }

  metric_query {
    id          = "silence"
    expression  = "IF(RATE(ended)>0 AND RATE(exported)==0, 1, 0)"
    label       = "spans ending but none exported"
    return_data = true
  }

  alarm_description = "er-${each.key} spans have been ending for 15 minutes with none exported — the #941 signature (a pod whose telemetry looks installed and exports nothing, self-healing only on restart). Check the er.telemetry.span_exports_failed counter and the pod log for 'OTLP span export batch failed'; if neither is present the wedge is in-process, between the batch processor and the wire. The divergence between the two input counters is the localization: ended>0/exported==0 = in-process, exports_failed rising = the collector or the network."

  alarm_actions = [local.alerts_topic_arn]
  ok_actions    = [local.alerts_topic_arn]
}
