# Task 0.5 (infrastructure.md §8.6): the two things that cannot be backfilled —
# an organization trail with log-file validation, delivered to a bucket in the
# management account (no workloads, no standing access) under compliance-mode
# Object Lock, so the principals a log records can never delete it.

locals {
  mgmt_account_id = "797781631727"
  org_id          = "o-bqzb54i69b"
  trail_name      = "er-org-trail"
  trail_arn       = "arn:aws:cloudtrail:us-east-2:${local.mgmt_account_id}:trail/${local.trail_name}"
}

resource "aws_kms_key" "audit" {
  description             = "Org CloudTrail archive (er-audit-archive)"
  enable_key_rotation     = true
  deletion_window_in_days = 30

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Sid       = "EnableIamPolicies"
        Effect    = "Allow"
        Principal = { AWS = "arn:aws:iam::${local.mgmt_account_id}:root" }
        Action    = "kms:*"
        Resource  = "*"
      },
      {
        Sid       = "CloudTrailEncrypt"
        Effect    = "Allow"
        Principal = { Service = "cloudtrail.amazonaws.com" }
        Action    = ["kms:GenerateDataKey*"]
        Resource  = "*"
        Condition = {
          StringEquals = { "aws:SourceArn" = local.trail_arn }
          StringLike = {
            "kms:EncryptionContext:aws:cloudtrail:arn" = "arn:aws:cloudtrail:*:${local.mgmt_account_id}:trail/*"
          }
        }
      },
      {
        Sid       = "CloudTrailDescribe"
        Effect    = "Allow"
        Principal = { Service = "cloudtrail.amazonaws.com" }
        Action    = ["kms:DescribeKey"]
        Resource  = "*"
      },
    ]
  })
}

resource "aws_kms_alias" "audit" {
  name          = "alias/er-audit-archive"
  target_key_id = aws_kms_key.audit.key_id
}

resource "aws_s3_bucket" "audit" {
  bucket              = "er-audit-archive-${local.mgmt_account_id}"
  object_lock_enabled = true

  lifecycle {
    prevent_destroy = true
  }
}

resource "aws_s3_bucket_versioning" "audit" {
  bucket = aws_s3_bucket.audit.id

  versioning_configuration {
    status = "Enabled"
  }
}

# ≥400 days: a year plus margin so a SOC 2 observation window never runs off
# the end (§8.6). COMPLIANCE mode: not even root shortens it.
resource "aws_s3_bucket_object_lock_configuration" "audit" {
  bucket = aws_s3_bucket.audit.id

  rule {
    default_retention {
      mode = "COMPLIANCE"
      days = 400
    }
  }

  depends_on = [aws_s3_bucket_versioning.audit]
}

resource "aws_s3_bucket_server_side_encryption_configuration" "audit" {
  bucket = aws_s3_bucket.audit.id

  rule {
    apply_server_side_encryption_by_default {
      sse_algorithm     = "aws:kms"
      kms_master_key_id = aws_kms_key.audit.arn
    }
    bucket_key_enabled = true
  }
}

resource "aws_s3_bucket_public_access_block" "audit" {
  bucket = aws_s3_bucket.audit.id

  block_public_acls       = true
  block_public_policy     = true
  ignore_public_acls      = true
  restrict_public_buckets = true
}

data "aws_iam_policy_document" "audit_bucket" {
  statement {
    sid     = "AWSCloudTrailAclCheck"
    effect  = "Allow"
    actions = ["s3:GetBucketAcl"]

    principals {
      type        = "Service"
      identifiers = ["cloudtrail.amazonaws.com"]
    }

    resources = [aws_s3_bucket.audit.arn]

    condition {
      test     = "StringEquals"
      variable = "aws:SourceArn"
      values   = [local.trail_arn]
    }
  }

  statement {
    sid     = "AWSCloudTrailWrite"
    effect  = "Allow"
    actions = ["s3:PutObject"]

    principals {
      type        = "Service"
      identifiers = ["cloudtrail.amazonaws.com"]
    }

    # Org trails deliver member-account logs under the org ID prefix and the
    # management account's own under its account ID.
    resources = [
      "${aws_s3_bucket.audit.arn}/AWSLogs/${local.mgmt_account_id}/*",
      "${aws_s3_bucket.audit.arn}/AWSLogs/${local.org_id}/*",
    ]

    condition {
      test     = "StringEquals"
      variable = "s3:x-amz-acl"
      values   = ["bucket-owner-full-control"]
    }

    condition {
      test     = "StringEquals"
      variable = "aws:SourceArn"
      values   = [local.trail_arn]
    }
  }

  statement {
    sid    = "DenyDeletesExceptRoot"
    effect = "Deny"

    actions = [
      "s3:DeleteObject",
      "s3:DeleteObjectVersion",
    ]

    resources = ["${aws_s3_bucket.audit.arn}/*"]

    principals {
      type        = "AWS"
      identifiers = ["*"]
    }

    condition {
      test     = "ArnNotLike"
      variable = "aws:PrincipalArn"
      values   = ["arn:aws:iam::${local.mgmt_account_id}:root"]
    }
  }

  statement {
    sid     = "DenyInsecureTransport"
    effect  = "Deny"
    actions = ["s3:*"]

    resources = [
      aws_s3_bucket.audit.arn,
      "${aws_s3_bucket.audit.arn}/*",
    ]

    principals {
      type        = "AWS"
      identifiers = ["*"]
    }

    condition {
      test     = "Bool"
      variable = "aws:SecureTransport"
      values   = ["false"]
    }
  }
}

resource "aws_s3_bucket_policy" "audit" {
  bucket = aws_s3_bucket.audit.id
  policy = data.aws_iam_policy_document.audit_bucket.json

  depends_on = [aws_s3_bucket_public_access_block.audit]
}

resource "aws_cloudtrail" "org" {
  name           = local.trail_name
  s3_bucket_name = aws_s3_bucket.audit.id
  kms_key_id     = aws_kms_key.audit.arn

  is_organization_trail         = true
  is_multi_region_trail         = true
  include_global_service_events = true
  enable_log_file_validation    = true

  depends_on = [aws_s3_bucket_policy.audit]
}
