# The production collector installed by #944 emits these bounded metrics.
# Missing snapshots alert rather than making an empty dashboard look healthy.
module "operations" {
  source           = "../../modules/operational-observability"
  environment      = "prod"
  region           = "us-east-2"
  metric_namespace = "ER/Prod"
  alerts_topic_arn = "arn:aws:sns:us-east-2:${data.aws_caller_identity.current.account_id}:er-alerts"
}
