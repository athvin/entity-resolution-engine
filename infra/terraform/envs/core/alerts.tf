# The notification path for CloudWatch alarms.
#
# Before this file there were **zero** `aws_cloudwatch_metric_alarm` and **zero**
# `aws_sns_*` resources in the whole repository. Every signal the account
# produced was something you had to go and look at: the two Budgets below email
# directly, and nothing else told anyone anything. That is the gap this closes,
# and it is a prerequisite rather than a feature — a CloudWatch alarm has no
# email action of its own, so an SNS topic is the only way an alarm can reach a
# person.
#
# ## Why the Budgets are NOT repointed at this topic
#
# The telemetry plan called for "one notification path", with the two existing
# Budgets repointed here. They are deliberately left on direct email:
#
#   - AWS Budgets is served from `us-east-1` (hence the `use1` provider alias in
#     versions.tf), while CloudWatch alarms can only publish to a topic in their
#     OWN region — `us-east-2` for everything here. One topic cannot serve both,
#     so "one path" would really be two topics that happen to share a
#     subscriber.
#   - With a single subscriber there is nothing to gain. SNS earns its keep when
#     a notification needs to fan out to more than one place, and the Budgets
#     already deliver to the address below.
#
# If a second destination is ever added — a pager, a webhook, a Slack relay —
# the right move is a `use1` topic beside this one and both Budgets repointed to
# it. Worth doing then, not now.
#
# NOTE: the exact Budgets-to-SNS region rule has not been verified against the
# AWS documentation here; the reasoning above stands on the CloudWatch side,
# which is certain, and on there being no behavioural gain either way.

resource "aws_sns_topic" "alerts" {
  name = "er-alerts"

  tags = {
    Purpose = "cloudwatch-alarm-notifications"
  }
}

resource "aws_sns_topic_subscription" "alerts_email" {
  topic_arn = aws_sns_topic.alerts.arn
  protocol  = "email"
  endpoint  = local.budget_alert_email

  # An email subscription is PENDING until the address confirms it, and
  # Terraform cannot confirm it. A first apply therefore leaves this alarm path
  # silent until someone clicks the link in the confirmation mail — which is
  # worth knowing, because an unconfirmed subscription looks identical to a
  # working one in the console's topic list.
}

# Without this, CloudWatch cannot publish and every alarm transitions silently.
# Scoped to this account so the topic cannot be used as a confused deputy by an
# alarm in someone else's.
data "aws_caller_identity" "current" {}

resource "aws_sns_topic_policy" "alerts" {
  arn = aws_sns_topic.alerts.arn

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Sid    = "AllowCloudWatchAlarmsToPublish"
        Effect = "Allow"
        Principal = {
          Service = "cloudwatch.amazonaws.com"
        }
        Action   = "SNS:Publish"
        Resource = aws_sns_topic.alerts.arn
        Condition = {
          StringEquals = {
            "AWS:SourceAccount" = data.aws_caller_identity.current.account_id
          }
        }
      },
    ]
  })
}

output "alerts_topic_arn" {
  description = "SNS topic every CloudWatch alarm in this account notifies. Consumed by envs/dev/alarms.tf."
  value       = aws_sns_topic.alerts.arn
}
