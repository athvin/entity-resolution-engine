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
variable "bindings" {
  type = map(object({
    namespace = string
    org_id    = string
    prefix    = string
  }))
  validation {
    condition = alltrue([for b in values(var.bindings) :
      length(trimspace(b.org_id)) > 0 &&
      can(regex("^[a-z0-9]([a-z0-9-]*[a-z0-9])?$", b.namespace)) &&
      length(b.namespace) <= 63 &&
      can(regex("^[A-Za-z0-9_-]+(/[A-Za-z0-9_-]+)*/$", b.prefix))
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
    condition     = length(distinct([for b in values(var.bindings) : "${b.namespace}/${b.org_id}"])) == length(var.bindings)
    error_message = "Each namespace/org pair may have only one runner identity."
  }
}

locals {
  accounts = { for key, b in var.bindings : key => "er-runner-${substr(sha256(b.org_id), 0, 20)}" }
  policies = { for key, b in var.bindings : key => jsonencode({
    Version = "2012-10-17"
    Statement = [
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
    ]
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

output "policies" { value = local.policies }
output "service_accounts" { value = local.accounts }
