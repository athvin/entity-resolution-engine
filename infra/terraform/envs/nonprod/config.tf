# Task 0.5's last piece: AWS Config with the §8.6 narrow scope — the resource
# types whose configuration history answers "was encryption on throughout the
# window". Evidence generator; not a compliance engine.

locals {
  nonprod_account_id = "660360495170"

  config_resource_types = [
    "AWS::S3::Bucket",
    "AWS::S3::AccountPublicAccessBlock",
    "AWS::KMS::Key",
    "AWS::RDS::DBCluster",
    "AWS::RDS::DBInstance",
    "AWS::EKS::Cluster",
    "AWS::SecretsManager::Secret",
    "AWS::EC2::SecurityGroup",
    "AWS::IAM::Role",
    "AWS::ECR::Repository",
  ]
}

resource "aws_iam_service_linked_role" "config" {
  aws_service_name = "config.amazonaws.com"
}

resource "aws_s3_bucket" "config" {
  bucket = "er-config-${local.nonprod_account_id}"
}

resource "aws_s3_bucket_public_access_block" "config" {
  bucket = aws_s3_bucket.config.id

  block_public_acls       = true
  block_public_policy     = true
  ignore_public_acls      = true
  restrict_public_buckets = true
}

data "aws_iam_policy_document" "config_bucket" {
  statement {
    sid     = "AWSConfigAclCheck"
    effect  = "Allow"
    actions = ["s3:GetBucketAcl", "s3:ListBucket"]

    principals {
      type        = "Service"
      identifiers = ["config.amazonaws.com"]
    }

    resources = [aws_s3_bucket.config.arn]

    condition {
      test     = "StringEquals"
      variable = "aws:SourceAccount"
      values   = [local.nonprod_account_id]
    }
  }

  statement {
    sid     = "AWSConfigWrite"
    effect  = "Allow"
    actions = ["s3:PutObject"]

    principals {
      type        = "Service"
      identifiers = ["config.amazonaws.com"]
    }

    resources = ["${aws_s3_bucket.config.arn}/AWSLogs/${local.nonprod_account_id}/Config/*"]

    condition {
      test     = "StringEquals"
      variable = "s3:x-amz-acl"
      values   = ["bucket-owner-full-control"]
    }

    condition {
      test     = "StringEquals"
      variable = "aws:SourceAccount"
      values   = [local.nonprod_account_id]
    }
  }

  statement {
    sid     = "DenyInsecureTransport"
    effect  = "Deny"
    actions = ["s3:*"]

    resources = [
      aws_s3_bucket.config.arn,
      "${aws_s3_bucket.config.arn}/*",
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

resource "aws_s3_bucket_policy" "config" {
  bucket = aws_s3_bucket.config.id
  policy = data.aws_iam_policy_document.config_bucket.json

  depends_on = [aws_s3_bucket_public_access_block.config]
}

resource "aws_config_configuration_recorder" "this" {
  name     = "er-config"
  role_arn = aws_iam_service_linked_role.config.arn

  recording_group {
    all_supported  = false
    resource_types = local.config_resource_types
  }
}

resource "aws_config_delivery_channel" "this" {
  name           = "er-config"
  s3_bucket_name = aws_s3_bucket.config.bucket

  depends_on = [aws_s3_bucket_policy.config, aws_config_configuration_recorder.this]
}

resource "aws_config_configuration_recorder_status" "this" {
  name       = aws_config_configuration_recorder.this.name
  is_enabled = true

  depends_on = [aws_config_delivery_channel.this]
}
