# Shared with the Helm chart: an org cannot acquire an AWS identity through
# a job parameter. CI applies this reviewed registry before the live probe.
locals {
  tenant_runner_registry = jsondecode(file("${path.module}/../../../k8s/charts/er-platform/tenant-runners.json"))["dev"]
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
  environment  = "dev"
  region       = "us-east-2"
  cluster_name = module.eks.cluster_name
  cluster_arn  = module.eks.cluster_arn
  bucket_arn   = module.lake.bucket_arn
  kms_key_arn  = module.lake.kms_key_arn
  bindings     = local.tenant_runner_bindings

  # The API pod streams an enrolled tenant's export back by assuming that
  # tenant's export reader (#1240).
  export_reader_principal_arn = aws_iam_role.er_api_pod.arn
}

# The API pod may assume exactly the registry's export readers, by ARN.
resource "aws_iam_role_policy" "er_api_export_readers" {
  count = length(module.tenant_runners.export_role_arns) == 0 ? 0 : 1
  name  = "assume-tenant-export-readers"
  role  = aws_iam_role.er_api_pod.id
  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Sid      = "AssumeTenantExportReaders"
      Effect   = "Allow"
      Action   = "sts:AssumeRole"
      Resource = module.tenant_runners.export_role_arns
    }]
  })
}
