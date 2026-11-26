# The deployed public API contract (#658, #1245): what production is serving,
# so a release can be refused BEFORE it breaks an external consumer.
#
# The API launch gate compares a candidate's `openapi.public.json` with the
# contract production last published, not with `origin/main`: main can move
# without a deploy, and a comparison against main proves nothing about what a
# customer is already integrated against. This bucket is that record.
#
#   releases/<sha>/openapi.public.json  — every published contract, written once
#                                         (the deploy puts it with If-None-Match: *)
#   deployed/openapi.public.json        — the one production is serving
#   deployed/release.json               — {sha, published_at}: which release that is
#
# SSE-S3, not KMS. The public document is public by construction (no operator
# route, no credential; it is also committed and served at /api/openapi), so
# KMS would only add the cross-stack key grants #658 named as the expensive
# part and protect nothing. Versioned, so an overwritten `deployed/` pointer is
# recoverable and the history of what was serving survives a bad publish.
#
# Applied in its own pull request BEFORE the deploy step that uses it, for the
# reason ci-deploy.tf's header gives: a PR carrying both would race the infra
# apply against the next deploy.

resource "aws_s3_bucket" "api_contract" {
  bucket = "er-prod-api-contract-797781631727"

  lifecycle {
    prevent_destroy = true
  }
}

resource "aws_s3_bucket_public_access_block" "api_contract" {
  bucket = aws_s3_bucket.api_contract.id

  block_public_acls       = true
  block_public_policy     = true
  ignore_public_acls      = true
  restrict_public_buckets = true
}

resource "aws_s3_bucket_server_side_encryption_configuration" "api_contract" {
  bucket = aws_s3_bucket.api_contract.id

  rule {
    apply_server_side_encryption_by_default {
      sse_algorithm = "AES256"
    }
  }
}

resource "aws_s3_bucket_versioning" "api_contract" {
  bucket = aws_s3_bucket.api_contract.id

  versioning_configuration {
    status = "Enabled"
  }
}

# Noncurrent `deployed/` pointers are history, not state: keep a year of them,
# then let them go. `releases/` objects are never overwritten, so they have no
# noncurrent versions to expire; current versions are never expired here.
resource "aws_s3_bucket_lifecycle_configuration" "api_contract" {
  bucket = aws_s3_bucket.api_contract.id

  rule {
    id     = "noncurrent-expiry"
    status = "Enabled"
    filter {}

    noncurrent_version_expiration {
      noncurrent_days = 365
    }

    abort_incomplete_multipart_upload {
      days_after_initiation = 7
    }
  }

  depends_on = [aws_s3_bucket_versioning.api_contract]
}

# TLS only, and no deletion of a published release by anyone: a release record
# that can be deleted is not a record. The pointer under `deployed/` is
# overwritten by design, so it is not covered.
data "aws_iam_policy_document" "api_contract_bucket" {
  statement {
    sid       = "DenyInsecureTransport"
    effect    = "Deny"
    actions   = ["s3:*"]
    resources = [aws_s3_bucket.api_contract.arn, "${aws_s3_bucket.api_contract.arn}/*"]

    principals {
      type        = "*"
      identifiers = ["*"]
    }

    condition {
      test     = "Bool"
      variable = "aws:SecureTransport"
      values   = ["false"]
    }
  }

  statement {
    sid       = "DenyReleaseDeletion"
    effect    = "Deny"
    actions   = ["s3:DeleteObject", "s3:DeleteObjectVersion"]
    resources = ["${aws_s3_bucket.api_contract.arn}/releases/*"]

    principals {
      type        = "*"
      identifiers = ["*"]
    }
  }
}

resource "aws_s3_bucket_policy" "api_contract" {
  bucket = aws_s3_bucket.api_contract.id
  policy = data.aws_iam_policy_document.api_contract_bucket.json

  depends_on = [aws_s3_bucket_public_access_block.api_contract]
}

# The prod deploy role reads the baseline before a release and publishes the
# new contract after cutover. Exactly the two prefixes; `ListBucket` scoped to
# them so a missing baseline reads as a 404 (first publication) rather than a
# 403 (which the gate must treat as unreadable, and so as a refusal).
data "aws_iam_policy_document" "ci_deploy_prod_api_contract" {
  statement {
    effect  = "Allow"
    actions = ["s3:GetObject", "s3:PutObject"]
    resources = [
      "${aws_s3_bucket.api_contract.arn}/deployed/*",
      "${aws_s3_bucket.api_contract.arn}/releases/*",
    ]
  }

  # Pull the release's own er-api image, so the gate runs the checker that
  # ships with it (`er-server api-check`) rather than a second implementation.
  # ci-deploy.tf's ECR statement resolves digests only; copying a binary out of
  # an image needs its layers too (#1248, found by the first gated deploy).
  # Scoped to the one repository the checker comes from.
  statement {
    effect = "Allow"
    actions = [
      "ecr:GetDownloadUrlForLayer",
      "ecr:BatchCheckLayerAvailability",
    ]
    resources = ["arn:aws:ecr:us-east-2:${data.aws_caller_identity.current.account_id}:repository/er-api"]
  }

  statement {
    effect    = "Allow"
    actions   = ["s3:ListBucket"]
    resources = [aws_s3_bucket.api_contract.arn]

    condition {
      test     = "StringLike"
      variable = "s3:prefix"
      values   = ["deployed/*", "releases/*"]
    }
  }
}

resource "aws_iam_role_policy" "ci_deploy_prod_api_contract" {
  name   = "api-contract"
  role   = aws_iam_role.ci_deploy_prod.id
  policy = data.aws_iam_policy_document.ci_deploy_prod_api_contract.json
}

output "api_contract_bucket" {
  value = aws_s3_bucket.api_contract.bucket
}
