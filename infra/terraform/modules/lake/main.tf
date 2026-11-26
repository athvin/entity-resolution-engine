# §7.3: the environment's lake bucket — SSE-KMS under its own CMK, TLS-only,
# versioned with the lifecycle the first draft omitted: every run rewrites
# parquet, so noncurrent versions expire at 30 days and incomplete multipart
# uploads at 7, or the bucket grows without bound and half-written runs bill
# silently.

variable "name" {
  type = string
}

# Reserved immutable artifact prefixes. No current-version expiration is
# configured: accepted deliveries and referenced results must survive retries.
variable "immutable_prefixes" {
  type    = list(string)
  default = []
  validation {
    condition = alltrue([for prefix in var.immutable_prefixes :
      can(regex("^[A-Za-z0-9_-]+/([A-Za-z0-9_*-]+/)+$", prefix))
    ])
    error_message = "Immutable prefixes must be scoped paths ending in '/'."
  }
}

resource "aws_kms_key" "lake" {
  description             = "${var.name} lake"
  enable_key_rotation     = true
  deletion_window_in_days = 30
}

resource "aws_kms_alias" "lake" {
  name          = "alias/${var.name}"
  target_key_id = aws_kms_key.lake.key_id
}

resource "aws_s3_bucket" "lake" {
  bucket = var.name

  lifecycle {
    prevent_destroy = true
  }
}

resource "aws_s3_bucket_versioning" "lake" {
  bucket = aws_s3_bucket.lake.id

  versioning_configuration {
    status = "Enabled"
  }
}

resource "aws_s3_bucket_server_side_encryption_configuration" "lake" {
  bucket = aws_s3_bucket.lake.id

  rule {
    apply_server_side_encryption_by_default {
      sse_algorithm     = "aws:kms"
      kms_master_key_id = aws_kms_key.lake.arn
    }
    bucket_key_enabled = true
  }
}

resource "aws_s3_bucket_public_access_block" "lake" {
  bucket = aws_s3_bucket.lake.id

  block_public_acls       = true
  block_public_policy     = true
  ignore_public_acls      = true
  restrict_public_buckets = true
}

resource "aws_s3_bucket_lifecycle_configuration" "lake" {
  bucket = aws_s3_bucket.lake.id

  rule {
    id     = "noncurrent-expiry"
    status = "Enabled"

    filter {}

    noncurrent_version_expiration {
      noncurrent_days = 30
    }

    abort_incomplete_multipart_upload {
      days_after_initiation = 7
    }
  }

  # Export artifacts (#1240) live under each tenant's `<prefix>exports/`, a
  # path no single prefix filter can name, so the writer tags them and this
  # rule matches the tag. The download refuses an export past its seven-day
  # retention by the job row regardless; this reclaims the bytes. The bucket is
  # versioned, so expiry only writes a delete marker and the noncurrent
  # version must expire too, or nothing is actually freed.
  rule {
    id     = "export-expiry"
    status = "Enabled"

    filter {
      tag {
        key   = "er-export"
        value = "1"
      }
    }

    expiration {
      days = 8
    }

    noncurrent_version_expiration {
      noncurrent_days = 1
    }
  }

  depends_on = [aws_s3_bucket_versioning.lake]
}

data "aws_iam_policy_document" "lake_tls_only" {
  dynamic "statement" {
    for_each = length(var.immutable_prefixes) == 0 ? [] : [1]
    content {
      sid       = "RequireImmutableConditionalPublication"
      effect    = "Deny"
      actions   = ["s3:PutObject"]
      resources = [for prefix in var.immutable_prefixes : "${aws_s3_bucket.lake.arn}/${prefix}*"]
      principals {
        type        = "*"
        identifiers = ["*"]
      }
      # Immutable files use bounded single PutObject, never multipart or COPY.
      # A conditional request plus deletion denial keeps the current key fixed.
      condition {
        test     = "Null"
        variable = "s3:if-none-match"
        values   = ["true"]
      }
    }
  }
  # The export-expiry lifecycle rule expires whatever carries er-export=1, so
  # an immutable artifact must never carry it, or tagging would be a way to
  # delete a sealed key on a timer (#1240).
  dynamic "statement" {
    for_each = length(var.immutable_prefixes) == 0 ? [] : [1]
    content {
      sid       = "ImmutableArtifactsNeverExpire"
      effect    = "Deny"
      actions   = ["s3:PutObject", "s3:PutObjectTagging"]
      resources = [for prefix in var.immutable_prefixes : "${aws_s3_bucket.lake.arn}/${prefix}*"]
      principals {
        type        = "*"
        identifiers = ["*"]
      }
      condition {
        test     = "StringEquals"
        variable = "s3:RequestObjectTag/er-export"
        values   = ["1"]
      }
    }
  }
  dynamic "statement" {
    for_each = length(var.immutable_prefixes) == 0 ? [] : [1]
    content {
      sid       = "RetainImmutableArtifacts"
      effect    = "Deny"
      actions   = ["s3:DeleteObject", "s3:DeleteObjectVersion"]
      resources = [for prefix in var.immutable_prefixes : "${aws_s3_bucket.lake.arn}/${prefix}*"]
      principals {
        type        = "*"
        identifiers = ["*"]
      }
    }
  }
  statement {
    sid     = "DenyInsecureTransport"
    effect  = "Deny"
    actions = ["s3:*"]

    resources = [
      aws_s3_bucket.lake.arn,
      "${aws_s3_bucket.lake.arn}/*",
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

resource "aws_s3_bucket_policy" "lake" {
  bucket = aws_s3_bucket.lake.id
  policy = data.aws_iam_policy_document.lake_tls_only.json

  depends_on = [aws_s3_bucket_public_access_block.lake]
}

output "bucket" {
  value = aws_s3_bucket.lake.bucket
}

output "bucket_arn" {
  value = aws_s3_bucket.lake.arn
}

output "kms_key_arn" {
  value = aws_kms_key.lake.arn
}
