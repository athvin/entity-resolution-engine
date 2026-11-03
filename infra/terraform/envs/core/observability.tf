# The span store, declared in `core` rather than per environment.
#
# There is ONE OpenObserve for every environment — dev, prod and local all ship
# to it, and the environment is a FIELD on the data
# (`deployment.environment.name`, already a resource attribute) rather than a
# separate stack. That follows the account model this repo already has: a single
# account where environments are separated by name prefix and permission set,
# not by account, so every guardrail is an IAM-layer deny. A per-environment
# observability stack would re-split exactly what that model treats as one
# place, and would mean maintaining two copies of the bucket, the IAM and the
# dashboards.
#
# This file holds only the AWS side — the bucket and the role. The Helm release
# lives in infra/k8s/observability/ and applies from CI like every other dev
# platform release (D-DEPLOY), because Terraform owning Kubernetes objects would
# be a second deploy mechanism for the same thing.
#
# ## The reason this can hold logs too
#
# §8.5 and D-OTEL rejected SaaS log backends because "logs are the signal that
# could carry record values" — which is why §10.5 splits the signals, logs to
# CloudWatch in-account and metrics/traces possibly out. A SELF-HOSTED store on
# our own S3 in our own account is in-account by construction, so that boundary
# does not apply to it and all three signals can share one destination. Without
# that, traces would live here and logs in CloudWatch: two places, which is the
# opposite of the point.

resource "aws_s3_bucket" "obs" {
  bucket = "er-obs-797781631727"

  # The span store. Not `prevent_destroy`: unlike the CUR (which cannot be
  # re-derived) telemetry is reproducible by re-running the thing that emitted
  # it, and a bucket that cannot be destroyed is a bucket that cannot be moved.
}

resource "aws_s3_bucket_public_access_block" "obs" {
  bucket = aws_s3_bucket.obs.id

  block_public_acls       = true
  block_public_policy     = true
  ignore_public_acls      = true
  restrict_public_buckets = true
}

resource "aws_s3_bucket_server_side_encryption_configuration" "obs" {
  bucket = aws_s3_bucket.obs.id

  rule {
    apply_server_side_encryption_by_default {
      sse_algorithm = "AES256"
    }
  }
}

resource "aws_s3_bucket_lifecycle_configuration" "obs" {
  bucket = aws_s3_bucket.obs.id

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

data "aws_iam_policy_document" "obs_pod_trust" {
  statement {
    effect  = "Allow"
    actions = ["sts:AssumeRole", "sts:TagSession"]

    principals {
      type        = "Service"
      identifiers = ["pods.eks.amazonaws.com"]
    }
  }
}

resource "aws_iam_role" "openobserve" {
  name               = "er-openobserve"
  assume_role_policy = data.aws_iam_policy_document.obs_pod_trust.json

  # Not `er-dev-openobserve`: this role is shared by every environment's store,
  # which is the whole point of decision 5. The name follows the thing, not the
  # environment that happens to reach it first.
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
    resources = ["${aws_s3_bucket.obs.arn}/*"]
  }

  statement {
    sid    = "SpanList"
    effect = "Allow"
    actions = [
      "s3:ListBucket",
      "s3:GetBucketLocation",
      "s3:ListBucketMultipartUploads",
    ]
    resources = [aws_s3_bucket.obs.arn]
  }
}

resource "aws_iam_role_policy" "openobserve" {
  name   = "span-store-objects"
  role   = aws_iam_role.openobserve.id
  policy = data.aws_iam_policy_document.openobserve.json
}

output "obs_bucket" {
  description = "The shared span store. Every environment's collector writes here."
  value       = aws_s3_bucket.obs.bucket
}

output "openobserve_role_arn" {
  description = "Bind this to the openobserve ServiceAccount with an aws_eks_pod_identity_association in the environment that runs the store."
  value       = aws_iam_role.openobserve.arn
}
