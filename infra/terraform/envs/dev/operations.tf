# The dev collector exports these instruments to ER/Dev. Production collector
# deployment remains #944; do not create apparently live prod metric coverage
# before that pipeline exists. The module is ready for that environment's wiring.
module "operations" {
  source           = "../../modules/operational-observability"
  environment      = "dev"
  region           = "us-east-2"
  metric_namespace = "ER/Dev"
  alerts_topic_arn = local.alerts_topic_arn
}
