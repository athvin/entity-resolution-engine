# The $300/$600 alarms (plan-to-aws.md 0.8). Created by CLI during bootstrap
# and imported here so they live under Terraform like everything else.

locals {
  budget_alert_email = "dupezone813@gmail.com"
}

resource "aws_budgets_budget" "monthly_300" {
  name         = "er-monthly-300"
  budget_type  = "COST"
  limit_amount = "300.0"
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

resource "aws_budgets_budget" "monthly_600" {
  name         = "er-monthly-600"
  budget_type  = "COST"
  limit_amount = "600.0"
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
