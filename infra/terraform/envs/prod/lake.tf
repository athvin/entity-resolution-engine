# Prod lake bucket (SSE-KMS under its own CMK, TLS-only, versioned,
# prevent_destroy — same module as dev). Unlike envs/dev/lake.tf there is NO
# static IAM-user access key here: prod reaches S3 via EKS Pod Identity (an IAM
# role), wired with the app deploy. §8.1 forbids human-minted long-lived keys,
# and prod honours that from day one (the dev static key is the Phase-1 interim
# only).
#
# "wired with the app deploy" was a forward reference for three weeks: the
# sentence above described a role that did not exist, so a prod pod had no path
# to this bucket at all. pod_identity.tf is that wiring, and the grants it
# attaches are the document below.
module "lake" {
  source = "../../modules/lake"

  name = "er-prod-lake-797781631727"
}

# The lake-object grants, attached to `er-prod-runner-pod` in pod_identity.tf.
# Deliberately the same shape as envs/dev/lake.tf's document — the difference
# between the environments is which bucket and which CMK, not which actions, so
# a prod-only action here would be a divergence to explain rather than a
# hardening step.
data "aws_iam_policy_document" "lake_access" {
  statement {
    sid     = "LakeObjects"
    effect  = "Allow"
    actions = ["s3:GetObject", "s3:PutObject", "s3:DeleteObject", "s3:AbortMultipartUpload", "s3:ListMultipartUploadParts"]

    resources = ["${module.lake.bucket_arn}/*"]
  }

  statement {
    sid       = "LakeList"
    effect    = "Allow"
    actions   = ["s3:ListBucket", "s3:GetBucketLocation", "s3:ListBucketMultipartUploads"]
    resources = [module.lake.bucket_arn]
  }

  statement {
    sid       = "LakeKey"
    effect    = "Allow"
    actions   = ["kms:Decrypt", "kms:GenerateDataKey*"]
    resources = [module.lake.kms_key_arn]
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
