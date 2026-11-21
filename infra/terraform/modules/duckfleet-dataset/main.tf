# Storage half of deployment-per-dataset isolation. Helm owns the namespace,
# catalog and credentials; Terraform alone owns Pod Identity and S3 grants.
terraform {
  required_version = ">= 1.11"
  required_providers {
    aws = { source = "hashicorp/aws", version = "~> 6.0" }
  }
}
variable "environment" { type = string }
variable "cluster_name" { type = string }
variable "cluster_arn" { type = string }
variable "bucket_arn" { type = string }
variable "kms_key_arn" { type = string }
variable "region" { type = string }
variable "datasets" {
  type = list(object({ org_id = string, dataset_id = string }))
  validation {
    condition = alltrue([for d in var.datasets :
      can(regex("^[A-Za-z0-9_-]{1,128}$", d.org_id)) &&
      can(regex("^[A-Za-z0-9_-]{1,128}$", d.dataset_id))
    ]) && length(distinct([for d in var.datasets : d.dataset_id])) == length(var.datasets)
    error_message = "Immutable IDs must be literal identifiers; each dataset has exactly one org owner."
  }
}
locals {
  # Matches er_duckfleet::namespace. Never derive a boundary from a display name.
  namespaces = { for d in var.datasets : d.dataset_id => "duckfleet-${substr(sha256("${d.org_id}\u0000${d.dataset_id}"), 0, 40)}" }
  bindings = { for d in var.datasets : d.dataset_id => {
    namespace          = local.namespaces[d.dataset_id]
    org_id             = "${d.org_id}/${d.dataset_id}"
    service_account    = "duckfleet-worker"
    prefix             = "duckfleet/${local.namespaces[d.dataset_id]}/"
    read_only_prefixes = ["duckfleet/${local.namespaces[d.dataset_id]}/artifacts/immutable/"]
  } }
}
module "identities" {
  providers    = { aws = aws }
  source       = "../tenant-runner"
  environment  = var.environment
  region       = var.region
  cluster_name = var.cluster_name
  cluster_arn  = var.cluster_arn
  bucket_arn   = var.bucket_arn
  kms_key_arn  = var.kms_key_arn
  bindings     = local.bindings
}
output "namespaces" { value = local.namespaces }
output "policies" { value = module.identities.policies }
output "service_accounts" { value = module.identities.service_accounts }
