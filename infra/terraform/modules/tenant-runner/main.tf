# One role and Pod Identity association for each (cluster, namespace, org).
# The registry is reviewed infrastructure, not an API-controlled role name.
terraform {
  required_version = ">= 1.11"
  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = "~> 6.0"
    }
  }
}

variable "environment" { type = string }
variable "cluster_name" { type = string }
variable "cluster_arn" { type = string }
variable "bucket_arn" { type = string }
variable "kms_key_arn" { type = string }
variable "region" { type = string }
# The er-api pod role, which alone may assume the per-tenant export readers
# below (#1240). Null creates none: a stack with no API identity cannot serve
# a download, and an export role nothing can assume is decoration.
variable "export_reader_principal_arn" {
  type    = string
  default = null
}
variable "bindings" {
  type = map(object({
    namespace          = string
    org_id             = string
    prefix             = string
    service_account    = optional(string)
    read_only_prefixes = optional(list(string), [])
  }))
  validation {
    condition = alltrue([for b in values(var.bindings) :
      length(trimspace(b.org_id)) > 0 &&
      can(regex("^[a-z0-9]([a-z0-9-]*[a-z0-9])?$", b.namespace)) &&
      length(b.namespace) <= 63 &&
      (b.service_account == null ? true : (can(regex("^[a-z0-9]([a-z0-9-]*[a-z0-9])?$", b.service_account)) && length(b.service_account) <= 63)) &&
      can(regex("^[A-Za-z0-9_-]+(/[A-Za-z0-9_-]+)*/$", b.prefix)) &&
      alltrue([for prefix in b.read_only_prefixes :
        startswith(prefix, b.prefix) &&
        can(regex("^[A-Za-z0-9_-]+(/[A-Za-z0-9_-]+)*/$", prefix))
      ])
    ])
    error_message = "Bindings require a nonempty org, a Kubernetes namespace and a literal non-root prefix ending in '/'; wildcards and path traversal are forbidden."
  }
  validation {
    condition = alltrue(flatten([for key, b in var.bindings : [
      for other_key, other in var.bindings : key == other_key ||
      (!startswith(b.prefix, other.prefix) && !startswith(other.prefix, b.prefix))
    ]]))
    error_message = "Tenant prefixes must not overlap, including across namespaces."
  }
  validation {
    condition     = length(distinct([for b in values(var.bindings) : "${b.namespace}/${coalesce(b.service_account, "er-runner-${substr(sha256(b.org_id), 0, 20)}")}"])) == length(var.bindings)
    error_message = "Each namespace/service-account pair may have only one runner identity."
  }
}

locals {
  accounts = { for key, b in var.bindings : key => coalesce(b.service_account, "er-runner-${substr(sha256(b.org_id), 0, 20)}") }
  policies = { for key, b in var.bindings : key => jsonencode({
    Version = "2012-10-17"
    Statement = concat([
      {
        Sid      = "TenantObjects"
        Effect   = "Allow"
        Action   = ["s3:GetObject", "s3:PutObject", "s3:DeleteObject", "s3:AbortMultipartUpload", "s3:ListMultipartUploadParts"]
        Resource = ["${var.bucket_arn}/${b.prefix}*"]
      },
      {
        # ListBucketMultipartUploads has no s3:prefix condition support;
        # whole-bucket upload discovery would cross the tenant boundary.
        # Per-object ListMultipartUploadParts remains available above.
        Sid       = "TenantList"
        Effect    = "Allow"
        Action    = ["s3:ListBucket"]
        Resource  = [var.bucket_arn]
        Condition = { StringLike = { "s3:prefix" = ["${b.prefix}*"] } }
      },
      {
        Sid      = "BucketLocation"
        Effect   = "Allow"
        Action   = ["s3:GetBucketLocation"]
        Resource = [var.bucket_arn]
      },
      {
        Sid      = "LakeKeyViaS3"
        Effect   = "Allow"
        Action   = ["kms:Decrypt", "kms:GenerateDataKey"]
        Resource = [var.kms_key_arn]
        # S3 Bucket Keys use the bucket ARN as the encryption context. The
        # tenant boundary is the S3 policy; direct KMS use is not granted.
        Condition = { StringEquals = {
          "kms:ViaService"                   = "s3.${var.region}.amazonaws.com"
          "kms:EncryptionContext:aws:s3:arn" = var.bucket_arn
        } }
      }
      ], length(b.read_only_prefixes) == 0 ? [] : [{
        Sid      = "ReadOnlyArtifacts"
        Effect   = "Deny"
        Action   = ["s3:PutObject", "s3:DeleteObject", "s3:DeleteObjectVersion", "s3:AbortMultipartUpload"]
        Resource = [for prefix in b.read_only_prefixes : "${var.bucket_arn}/${prefix}*"]
      }],
      # Last, so the statement indices other modules' tests assert stay put.
      # The export writer tags its artifact so the lake's tag-filtered
      # lifecycle rule expires it (#1240); a tagged PutObject needs this too.
      [{
        Sid      = "TenantExportTagging"
        Effect   = "Allow"
        Action   = ["s3:PutObjectTagging"]
        Resource = ["${var.bucket_arn}/${b.prefix}exports/*"]
    }])
  }) }
}

resource "aws_iam_role" "tenant" {
  for_each = var.bindings
  name     = "er-${var.environment}-runner-${substr(sha256(each.key), 0, 20)}"
  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect    = "Allow"
      Principal = { Service = "pods.eks.amazonaws.com" }
      Action    = ["sts:AssumeRole", "sts:TagSession"]
      Condition = { StringEquals = {
        "aws:RequestTag/eks-cluster-arn"            = var.cluster_arn
        "aws:RequestTag/kubernetes-namespace"       = each.value.namespace
        "aws:RequestTag/kubernetes-service-account" = local.accounts[each.key]
      } }
    }]
  })
}

resource "aws_iam_role_policy" "lake" {
  for_each = var.bindings
  name     = "tenant-lake"
  role     = aws_iam_role.tenant[each.key].id
  policy   = local.policies[each.key]
}

resource "aws_eks_pod_identity_association" "tenant" {
  for_each             = var.bindings
  cluster_name         = var.cluster_name
  namespace            = each.value.namespace
  service_account      = local.accounts[each.key]
  role_arn             = aws_iam_role.tenant[each.key].arn
  disable_session_tags = false
}

# One export READER per binding (#1240). The API pod streams a tenant's export
# back by assuming THAT tenant's reader, chosen from the dataset's org, so a
# download for org A cannot read org B's artifact even if the handler's guard
# were wrong: IAM refuses it. Read-only, on `<prefix>exports/*` only, never the
# lake data beside it.
locals {
  export_policies = { for key, b in var.bindings : key => jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Sid      = "TenantExportRead"
        Effect   = "Allow"
        Action   = ["s3:GetObject"]
        Resource = ["${var.bucket_arn}/${b.prefix}exports/*"]
      },
      {
        Sid      = "LakeKeyViaS3"
        Effect   = "Allow"
        Action   = ["kms:Decrypt"]
        Resource = [var.kms_key_arn]
        Condition = { StringEquals = {
          "kms:ViaService"                   = "s3.${var.region}.amazonaws.com"
          "kms:EncryptionContext:aws:s3:arn" = var.bucket_arn
        } }
      }
    ]
  }) }
  export_bindings = var.export_reader_principal_arn == null ? {} : var.bindings
}

# Named by the same recipe the chart uses to tell er-api which ARN serves
# which org: er-<env>-export-<sha256("<namespace>/<org>")[:20]>.
resource "aws_iam_role" "export" {
  for_each = local.export_bindings
  name     = "er-${var.environment}-export-${substr(sha256(each.key), 0, 20)}"
  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect    = "Allow"
      Principal = { AWS = var.export_reader_principal_arn }
      Action    = "sts:AssumeRole"
    }]
  })
}

resource "aws_iam_role_policy" "export" {
  for_each = local.export_bindings
  name     = "tenant-export-read"
  role     = aws_iam_role.export[each.key].id
  policy   = local.export_policies[each.key]
}

output "policies" { value = local.policies }
output "export_policies" { value = local.export_policies }
output "export_role_arns" { value = [for r in aws_iam_role.export : r.arn] }
output "service_accounts" { value = local.accounts }
