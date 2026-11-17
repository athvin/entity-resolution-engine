# The two-tier cost ladder (plan-to-aws.md 0.8). Created by CLI during bootstrap
# and imported here so they live under Terraform like everything else.
#
# The tiers are named for their ROLE, not their amount. They were originally
# `er-monthly-300` / `er-monthly-600`, which went stale the moment the floor
# moved: by 2026-11 the steady-state run rate had passed the upper tier, so the
# "something is wrong" alarm fired on ordinary operation and stopped carrying
# information. The amount now lives in exactly one place — `limit_amount` — and
# re-baselining is a one-line edit that cannot leave a misleading name behind.
#
# Re-baselined 2026-11-16 against measured spend. The floor is the cost of two
# complete environments, which is mostly fixed: 2 EKS control planes (~$146/mo),
# 2 Auto Mode system nodes (~$96), 2 NAT gateways (~$64), 5 public IPv4
# addresses (~$18).
#
#   measured:  2026-11-10..15 averaged $23.08/day of post-prod-launch
#              steady state  ->  ~$702/month
#   watch:     $800   (~14% over the floor — meaningful drift, not noise)
#   alarm:     $1000  (~43% over the floor — something is actually wrong)
#
# Re-measure before changing these: `aws ce get-cost-and-usage --granularity
# DAILY --metrics UnblendedCost --group-by Type=DIMENSION,Key=SERVICE` over a
# window with no known anomaly in it. November 2026 is a bad window to read —
# $229 of it was one-off S3 egress from sccache, which ended on the 9th when the
# cache moved to R2 (#920).

locals {
  budget_alert_email = "dupezone813@gmail.com"
}

# Tier 1 — "look at this." Crossing it is drift worth a glance, not an incident.
resource "aws_budgets_budget" "monthly_watch" {
  name         = "er-monthly-watch"
  budget_type  = "COST"
  limit_amount = "800.0"
  limit_unit   = "USD"
  time_unit    = "MONTHLY"

  notification {
    notification_type          = "ACTUAL"
    comparison_operator        = "GREATER_THAN"
    threshold                  = 100
    threshold_type             = "PERCENTAGE"
    subscriber_email_addresses = [local.budget_alert_email]
  }

  notification {
    notification_type          = "FORECASTED"
    comparison_operator        = "GREATER_THAN"
    threshold                  = 100
    threshold_type             = "PERCENTAGE"
    subscriber_email_addresses = [local.budget_alert_email]
  }
}

# Tier 2 — "something is wrong." Crossing it means the floor itself has moved,
# or something is running that nobody intended.
resource "aws_budgets_budget" "monthly_alarm" {
  name         = "er-monthly-alarm"
  budget_type  = "COST"
  limit_amount = "1000.0"
  limit_unit   = "USD"
  time_unit    = "MONTHLY"

  notification {
    notification_type          = "ACTUAL"
    comparison_operator        = "GREATER_THAN"
    threshold                  = 100
    threshold_type             = "PERCENTAGE"
    subscriber_email_addresses = [local.budget_alert_email]
  }

  notification {
    notification_type          = "FORECASTED"
    comparison_operator        = "GREATER_THAN"
    threshold                  = 100
    threshold_type             = "PERCENTAGE"
    subscriber_email_addresses = [local.budget_alert_email]
  }
}
