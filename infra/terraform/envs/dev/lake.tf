# §7.3 + §9.2: the dev lake and the Phase 1 interim credential — one IAM
# service user with a lake-scoped policy, keys kept in Secrets Manager
# (er/dev/shared/s3) and consumed by scripts/dev_env.sh at dev-up. Retired at
# 4.1 when DuckDB/boto3 move to the credential chain under IRSA.

module "lake" {
  source = "../../modules/lake"

  name = "er-dev-lake-797781631727"
}

resource "aws_iam_user" "s3" {
  name = "er-dev-s3"
}

data "aws_iam_policy_document" "s3_user" {
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

resource "aws_iam_user_policy" "s3" {
  name   = "er-dev-lake"
  user   = aws_iam_user.s3.name
  policy = data.aws_iam_policy_document.s3_user.json
}

resource "aws_iam_access_key" "s3" {
  user = aws_iam_user.s3.name
}

resource "aws_secretsmanager_secret" "s3" {
  name        = "er/dev/shared/s3"
  description = "The 9.2 interim lake credential; consumed by scripts/dev_env.sh. Phase 4 retires it."
}

resource "aws_secretsmanager_secret_version" "s3" {
  secret_id = aws_secretsmanager_secret.s3.id

  secret_string = jsonencode({
    ER_S3_ACCESS_KEY_ID     = aws_iam_access_key.s3.id
    ER_S3_SECRET_ACCESS_KEY = aws_iam_access_key.s3.secret
    ER_S3_ENDPOINT          = "s3.us-east-2.amazonaws.com"
    ER_S3_REGION            = "us-east-2"
    ER_S3_URL_STYLE         = "vhost"
    ER_S3_USE_SSL           = "true"
    LAKE_BUCKET             = module.lake.bucket
  })
}

output "lake_bucket" {
  value = module.lake.bucket
}
