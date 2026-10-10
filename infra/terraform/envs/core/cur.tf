# Task 0.6 (plan-to-aws.md): CUR delivery before anything billable exists —
# billing data never backfills. CUR 2.0 via Data Exports: hourly, Parquet,
# per-resource rows, and Split Cost Allocation Data (the per-pod rows that
# make §16.1's per-tenant attribution possible). cur-query.sql is generated
# from the Data Exports table schema (all 127 columns incl. split_line_item_*).

resource "aws_s3_bucket" "cur" {
  bucket = "er-cur-797781631727"

  lifecycle {
    prevent_destroy = true
  }
}

resource "aws_s3_bucket_public_access_block" "cur" {
  bucket = aws_s3_bucket.cur.id

  block_public_acls       = true
  block_public_policy     = true
  ignore_public_acls      = true
  restrict_public_buckets = true
}

data "aws_iam_policy_document" "cur_delivery" {
  statement {
    sid    = "AllowDataExportsDelivery"
    effect = "Allow"

    principals {
      type = "Service"
      identifiers = [
        "bcm-data-exports.amazonaws.com",
        "billingreports.amazonaws.com",
      ]
    }

    actions = [
      "s3:PutObject",
      "s3:GetBucketPolicy",
    ]

    resources = [
      aws_s3_bucket.cur.arn,
      "${aws_s3_bucket.cur.arn}/*",
    ]

    condition {
      test     = "StringLike"
      variable = "aws:SourceAccount"
      values   = ["797781631727"]
    }

    condition {
      test     = "StringLike"
      variable = "aws:SourceArn"
      values = [
        "arn:aws:cur:us-east-1:797781631727:definition/*",
        "arn:aws:bcm-data-exports:us-east-1:797781631727:export/*",
      ]
    }
  }

  statement {
    sid     = "DenyInsecureTransport"
    effect  = "Deny"
    actions = ["s3:*"]

    resources = [
      aws_s3_bucket.cur.arn,
      "${aws_s3_bucket.cur.arn}/*",
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

resource "aws_s3_bucket_policy" "cur" {
  bucket = aws_s3_bucket.cur.id
  policy = data.aws_iam_policy_document.cur_delivery.json

  depends_on = [aws_s3_bucket_public_access_block.cur]
}

resource "aws_bcmdataexports_export" "cur2" {
  provider = aws.use1

  export {
    name = "er-cur2"

    data_query {
      query_statement = file("${path.module}/cur-query.sql")

      table_configurations = {
        COST_AND_USAGE_REPORT = {
          # The API injects BILLING_VIEW_ARN into its response; stating it
          # explicitly keeps plans clean (aws provider inconsistency otherwise).
          BILLING_VIEW_ARN                      = "arn:aws:billing::797781631727:billingview/primary"
          TIME_GRANULARITY                      = "HOURLY"
          INCLUDE_RESOURCES                     = "TRUE"
          INCLUDE_SPLIT_COST_ALLOCATION_DATA    = "TRUE"
          INCLUDE_MANUAL_DISCOUNT_COMPATIBILITY = "FALSE"
        }
      }
    }

    destination_configurations {
      s3_destination {
        s3_bucket = aws_s3_bucket.cur.bucket
        s3_prefix = "cur2"
        s3_region = "us-east-2"

        s3_output_configurations {
          overwrite   = "OVERWRITE_REPORT"
          format      = "PARQUET"
          compression = "PARQUET"
          output_type = "CUSTOM"
        }
      }
    }

    refresh_cadence {
      frequency = "SYNCHRONOUS"
    }
  }

  depends_on = [aws_s3_bucket_policy.cur]
}
