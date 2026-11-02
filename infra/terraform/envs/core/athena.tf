# Make the CUR queryable (infrastructure.md §16.1: "anything programmatic — a
# per-tenant monthly figure, a cost-per-million-records trend — needs the CUR
# delivered to S3 and queried with Athena").
#
# cur.tf has been delivering CUR 2.0 with Split Cost Allocation Data since
# Phase 0 and nothing has ever read it. This is the read side: a Glue table over
# the existing export, an Athena workgroup with a scan cap, and a results bucket
# that expires.
#
# Three things here are load-bearing and easy to get wrong:
#
#   1. PARTITION PROJECTION, not a crawler and not ALTER TABLE ADD PARTITION.
#      §16.1: "Partition it, or Athena scans the whole history on every query."
#      Projection computes the partition list from a rule, so there is no
#      metadata to keep fresh and no crawler to pay for.
#   2. A COLUMN SUBSET, resolved by name. cur-query.sql selects all 127 columns;
#      declaring all 127 here would be a maintenance liability for columns
#      nothing reads. `parquet.column.index.access = "false"` makes Parquet
#      resolve by name, which is what makes a subset correct rather than
#      silently misaligned.
#   3. A SCAN CAP on the workgroup. Athena bills per byte scanned, so a query
#      that forgets `WHERE billing_period = ...` is a bill, not an error. The
#      cap turns it back into an error.
#
# The export's cadence is OVERWRITE_REPORT (cur.tf), so each billing period's
# partition is replaced wholesale — there is no dedupe-by-identity_line_item_id
# step to write, which the legacy CUR 1.0 CREATE_NEW_REPORT mode would have
# needed.

locals {
  # Where cur.tf's export lands: bucket/<s3_prefix>/<export name>/data/.
  cur_data_prefix = "cur2/er-cur2/data"

  # Projection needs a floor, and it cannot be "the beginning of time" or every
  # query enumerates dead months. The account's first billing period.
  cur_first_billing_period = "2026-10"
}

# --- query results ----------------------------------------------------------

resource "aws_s3_bucket" "athena_results" {
  bucket = "er-athena-results-797781631727"
}

resource "aws_s3_bucket_public_access_block" "athena_results" {
  bucket = aws_s3_bucket.athena_results.id

  block_public_acls       = true
  block_public_policy     = true
  ignore_public_acls      = true
  restrict_public_buckets = true
}

resource "aws_s3_bucket_server_side_encryption_configuration" "athena_results" {
  bucket = aws_s3_bucket.athena_results.id

  rule {
    apply_server_side_encryption_by_default {
      sse_algorithm = "AES256"
    }
  }
}

resource "aws_s3_bucket_lifecycle_configuration" "athena_results" {
  bucket = aws_s3_bucket.athena_results.id

  rule {
    id     = "expire-query-results"
    status = "Enabled"
    filter {}

    # A result is a cache of a deterministic query over an immutable partition:
    # re-running it costs a scan, not a re-derivation. Long enough to re-read a
    # result within a session, short enough that this bucket never becomes a
    # cost line of its own.
    expiration {
      days = 14
    }

    abort_incomplete_multipart_upload {
      days_after_initiation = 7
    }
  }
}

# --- the workgroup ----------------------------------------------------------

resource "aws_athena_workgroup" "cost" {
  name        = "er-cost"
  description = "Cost and usage queries over the CUR 2.0 export (infrastructure.md §16)."

  configuration {
    # A query cannot opt out of the cap or redirect its output.
    enforce_workgroup_configuration = true

    # Publishes DataScannedInBytes / QueryState to CloudWatch, which is what the
    # cost model's own cost gets alarmed on (§16.3's levers apply to the
    # measurement too, not just the thing measured).
    publish_cloudwatch_metrics_enabled = true

    # 10 GiB. Every intended query filters on billing_period and scans a small
    # fraction of one month; anything approaching this forgot its WHERE, and
    # failing is cheaper than succeeding.
    bytes_scanned_cutoff_per_query = 10 * 1024 * 1024 * 1024

    result_configuration {
      output_location = "s3://${aws_s3_bucket.athena_results.bucket}/q/"

      encryption_configuration {
        encryption_option = "SSE_S3"
      }
    }
  }
}

# --- the catalog ------------------------------------------------------------

resource "aws_glue_catalog_database" "cur" {
  name        = "er_cur"
  description = "Cost and Usage Report 2.0, delivered by cur.tf."
}

resource "aws_glue_catalog_table" "cur2" {
  name          = "cur2"
  database_name = aws_glue_catalog_database.cur.name
  table_type    = "EXTERNAL_TABLE"
  description   = "CUR 2.0 hourly, per-resource, with split cost allocation. Partitioned by billing period via projection."

  parameters = {
    EXTERNAL       = "TRUE"
    classification = "parquet"

    # Resolve Parquet columns BY NAME. This is what makes declaring a subset of
    # the export's 127 columns safe — by position, a subset would silently read
    # the wrong data.
    "parquet.column.index.access" = "false"

    # Partition projection: the partition list is computed, never registered.
    "projection.enabled"                      = "true"
    "projection.billing_period.type"          = "date"
    "projection.billing_period.format"        = "yyyy-MM"
    "projection.billing_period.range"         = "${local.cur_first_billing_period},NOW"
    "projection.billing_period.interval"      = "1"
    "projection.billing_period.interval.unit" = "MONTHS"

    # $${} escapes Terraform interpolation: Athena needs the literal
    # ${billing_period} placeholder in the template.
    "storage.location.template" = "s3://${aws_s3_bucket.cur.bucket}/${local.cur_data_prefix}/BILLING_PERIOD=$${billing_period}"
  }

  partition_keys {
    name = "billing_period"
    type = "string"
  }

  storage_descriptor {
    location      = "s3://${aws_s3_bucket.cur.bucket}/${local.cur_data_prefix}/"
    input_format  = "org.apache.hadoop.hive.ql.io.parquet.MapredParquetInputFormat"
    output_format = "org.apache.hadoop.hive.ql.io.parquet.MapredParquetOutputFormat"

    ser_de_info {
      serialization_library = "org.apache.hadoop.hive.ql.io.parquet.serde.ParquetHiveSerDe"
    }

    # ~30 of the export's 127 columns: the identity, the line item, the product
    # and pricing shape, and the whole split_line_item_* family that makes
    # per-pod attribution possible (§16.1). Add a column here when a query needs
    # it; cur-query.sql remains the full list of what is delivered.

    # identity + line item
    columns {
      name = "identity_line_item_id"
      type = "string"
    }
    columns {
      name = "line_item_usage_start_date"
      type = "timestamp"
    }
    columns {
      name = "line_item_usage_end_date"
      type = "timestamp"
    }
    columns {
      name = "line_item_line_item_type"
      type = "string"
    }
    columns {
      name = "line_item_product_code"
      type = "string"
    }
    columns {
      name = "line_item_usage_type"
      type = "string"
    }
    columns {
      name = "line_item_operation"
      type = "string"
    }
    columns {
      name = "line_item_resource_id"
      type = "string"
    }
    columns {
      name = "line_item_usage_amount"
      type = "double"
    }
    columns {
      name = "line_item_unblended_cost"
      type = "double"
    }
    columns {
      name = "line_item_availability_zone"
      type = "string"
    }

    # product + pricing: what the resource was and how it was bought, which is
    # how the per-class blended rate gets calibrated.
    columns {
      name = "product_servicecode"
      type = "string"
    }
    columns {
      name = "product_instance_type"
      type = "string"
    }
    columns {
      name = "product_instance_family"
      type = "string"
    }
    columns {
      name = "product_region_code"
      type = "string"
    }
    columns {
      name = "pricing_purchase_option"
      type = "string"
    }
    columns {
      name = "pricing_term"
      type = "string"
    }
    columns {
      name = "pricing_public_on_demand_cost"
      type = "double"
    }

    # Split Cost Allocation Data — the per-pod rows. INCLUDE_SPLIT_COST_
    # ALLOCATION_DATA is TRUE in cur.tf, which is what puts these here.
    #
    # unused_cost is NOT optional to read: a runner pod holds a whole node
    # (taint + do-not-disrupt + WhenEmpty + inFlightCap 1 + the 8-vCPU account
    # ceiling), so the unused share is large and dropping it understates a run.
    columns {
      name = "split_line_item_split_usage_ratio"
      type = "double"
    }
    columns {
      name = "split_line_item_actual_usage"
      type = "double"
    }
    columns {
      name = "split_line_item_reserved_usage"
      type = "double"
    }
    columns {
      name = "split_line_item_split_usage"
      type = "double"
    }
    columns {
      name = "split_line_item_split_cost"
      type = "double"
    }
    columns {
      name = "split_line_item_unused_cost"
      type = "double"
    }
    columns {
      name = "split_line_item_parent_resource_id"
      type = "string"
    }

    # Tag and cost-category maps. resource_tags carries the AWS-generated
    # aws:eks:* set once those are activated as cost allocation tags; arbitrary
    # pod labels do NOT appear here, which is why the pod -> run join goes
    # through task_instances rather than a label.
    columns {
      name = "resource_tags"
      type = "map<string,string>"
    }
    columns {
      name = "cost_category"
      type = "map<string,string>"
    }
  }
}
