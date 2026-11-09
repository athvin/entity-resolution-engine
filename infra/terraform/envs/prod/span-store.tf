# Dedicated production telemetry store, selected by the owner for #944.
# Production data and metadata never traverse the development VPC or bucket.
# The deployment mounts the same checksum-pinned RDS trust bundle as the app.

resource "aws_s3_bucket" "prod_obs" {
  bucket = "er-prod-obs-797781631727"

  lifecycle {
    prevent_destroy = true
  }
}

resource "aws_s3_bucket_public_access_block" "prod_obs" {
  bucket = aws_s3_bucket.prod_obs.id

  block_public_acls       = true
  block_public_policy     = true
  ignore_public_acls      = true
  restrict_public_buckets = true
}

resource "aws_s3_bucket_server_side_encryption_configuration" "prod_obs" {
  bucket = aws_s3_bucket.prod_obs.id

  rule {
    apply_server_side_encryption_by_default {
      sse_algorithm = "AES256"
    }
  }
}

resource "aws_s3_bucket_lifecycle_configuration" "prod_obs" {
  bucket = aws_s3_bucket.prod_obs.id

  # Retention lives here rather than in the application, because this is where
  # it is enforceable. OpenObserve's own retention setting deletes from its
  # index; the lifecycle rule deletes the bytes.
  #
  # 30 days matches the §10.4 debug tier that `/er/dev/containers` already
  # uses. The audit tier (400 days, D-RETENTION) is deliberately NOT here:
  # those trails are CloudTrail in core and Postgres tables, and a compliance
  # artifact should not depend on a service we operate.
  rule {
    id     = "expire-telemetry"
    status = "Enabled"
    filter {}

    expiration {
      days = 30
    }

    # Parquet is written in multipart; an interrupted ingester otherwise leaves
    # parts that are billed and invisible.
    abort_incomplete_multipart_upload {
      days_after_initiation = 3
    }
  }
}

# --- the store's own identity -----------------------------------------------

resource "aws_iam_role" "openobserve" {
  name               = "er-prod-openobserve"
  assume_role_policy = data.aws_iam_policy_document.pod_trust.json

}

data "aws_iam_policy_document" "openobserve" {
  statement {
    sid    = "SpanObjects"
    effect = "Allow"
    actions = [
      "s3:GetObject",
      "s3:PutObject",
      "s3:DeleteObject",
      "s3:AbortMultipartUpload",
      "s3:ListMultipartUploadParts",
    ]
    resources = ["${aws_s3_bucket.prod_obs.arn}/*"]
  }

  statement {
    sid    = "SpanList"
    effect = "Allow"
    actions = [
      "s3:ListBucket",
      "s3:GetBucketLocation",
      "s3:ListBucketMultipartUploads",
    ]
    resources = [aws_s3_bucket.prod_obs.arn]
  }
}

resource "aws_iam_role_policy" "openobserve" {
  name   = "span-store-objects"
  role   = aws_iam_role.openobserve.id
  policy = data.aws_iam_policy_document.openobserve.json
}


resource "aws_eks_pod_identity_association" "openobserve" {
  cluster_name    = module.eks.cluster_name
  namespace       = "observability"
  service_account = "openobserve"
  role_arn        = aws_iam_role.openobserve.arn
}


resource "random_password" "openobserve_root" {
  length = 32
  # The chart's own default is "Complexpass#123"; OpenObserve requires a
  # mixed-class password, so the character set is pinned rather than left to
  # chance on a regenerate.
  override_special = "!#$%*()-_=+[]{}<>:?"
  min_upper        = 2
  min_lower        = 2
  min_numeric      = 2
  min_special      = 2
}

resource "aws_secretsmanager_secret" "openobserve" {
  name        = "er/prod/openobserve"
  description = "Root credentials and metadata DSN for the production span store; consumed by scripts/obs_up.sh (and the CI deploy that calls it)."
}

resource "aws_secretsmanager_secret_version" "openobserve" {
  secret_id = aws_secretsmanager_secret.openobserve.id

  secret_string = jsonencode({
    root_email    = "observability@dupezero.com"
    root_password = random_password.openobserve_root.result
    # URI form, which is what OpenObserve's ZO_META_POSTGRES_DSN expects —
    # not the libpq keyword form the control plane uses.
    meta_dsn = "postgres://er:${random_password.db.result}@${aws_rds_cluster.prod.endpoint}:5432/er?sslmode=verify-full&sslrootcert=/etc/ssl/rds/global-bundle.pem"
  })
}

