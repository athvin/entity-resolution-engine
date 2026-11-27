variable "environment" {
  type = string
}
variable "region" {
  type = string
}
variable "metric_namespace" {
  type = string
}
variable "alerts_topic_arn" {
  type = string
}
variable "queue_threshold" {
  type        = number
  default     = 100
  description = "Page on this many waiting jobs sustained for 15 minutes; tune against observed arrival and service rates."
}
variable "failure_rate_threshold" {
  type        = number
  default     = 0.25
  description = "Page when failed task attempts exceed this share of finished attempts over an hour."
}
variable "failure_rate_min_attempts" {
  type        = number
  default     = 4
  description = "Finished attempts an hour must hold before the failure rate can page, so one failure out of one never does."
}
variable "spot_interruptions_threshold" {
  type        = number
  default     = 5
  description = "Page when more EC2 Spot interruption warnings than this arrive in an hour."
}
