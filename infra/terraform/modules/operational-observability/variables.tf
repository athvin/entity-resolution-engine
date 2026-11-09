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
