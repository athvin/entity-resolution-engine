# Shared with the Helm chart: an org cannot acquire an AWS identity through
# a job parameter. CI applies this reviewed registry before the live probe.
locals {
  tenant_runner_registry = jsondecode(file("${path.module}/../../../k8s/charts/er-platform/tenant-runners.json"))["prod"]
  tenant_runner_bindings = merge({}, [for namespace, orgs in local.tenant_runner_registry : {
    for org, prefix in orgs : "${namespace}/${org}" => {
      namespace = namespace
      org_id    = org
      prefix    = prefix
    }
  }]...)
}

module "tenant_runners" {
  source       = "../../modules/tenant-runner"
  environment  = "prod"
  region       = "us-east-2"
  cluster_name = module.eks.cluster_name
  cluster_arn  = module.eks.cluster_arn
  bucket_arn   = module.lake.bucket_arn
  kms_key_arn  = module.lake.kms_key_arn
  bindings     = local.tenant_runner_bindings
}
