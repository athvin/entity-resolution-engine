# The environment lake: SSE-KMS, TLS-only and versioned. The shared runner
# identity has no data permissions; tenant_runners.tf owns prefix-scoped roles.
module "lake" {
  source = "../../modules/lake"

  name = "er-prod-lake-797781631727"
}

# The shared fallback is deliberately unable to read/write/list lake data.
# Tenant grants live only on the registry-bound roles in tenant_runners.tf.
# Keep this policy in place so existing role/association addresses survive the
# rollout; unregistered tenants retain local/EFS execution without AWS data.
data "aws_iam_policy_document" "lake_access" {
  statement {
    sid       = "BucketLocationOnly"
    effect    = "Allow"
    actions   = ["s3:GetBucketLocation"]
    resources = [module.lake.bucket_arn]
  }
}

output "lake_bucket" {
  value = module.lake.bucket
}

output "lake_bucket_arn" {
  value = module.lake.bucket_arn
}

output "lake_kms_key_arn" {
  value = module.lake.kms_key_arn
}
