# §7.3: the dev lake bucket, and the IAM policy document describing access to
# it.
#
# §9.2's interim credential — one IAM service user, a never-expiring access key
# and an `er/dev/shared/s3` Secrets Manager entry carrying it — is gone, which
# is 4.1's S3 half. It could go because nothing consumed it: the Rust engine has
# no `ER_S3_*` reader, `er-server`'s `secret://` resolver was Python's and was
# deleted with the rest, and the one real S3 client in the tree
# (`er-server`'s `s3_ingest::Watcher`) builds from the default credential chain.
#
# The grants below survive the user, attached to the `er-dev-runner-pod` Pod
# Identity role in pod_identity.tf. That role is how the engine reaches the lake
# once it can open an `s3://` one at all — which it cannot yet; the
# ducklake-lakehouse skill records that gap, and closing it is a feature, not a
# config change.

module "lake" {
  source = "../../modules/lake"

  name = "er-dev-lake-797781631727"
}

# The lake-object grants. Attached to a role rather than a user — the name says
# what they allow, not who holds them, because the holder is now Pod Identity.
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
