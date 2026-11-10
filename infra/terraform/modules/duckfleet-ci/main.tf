terraform {
  required_version = ">= 1.11"
  required_providers { aws = { source = "hashicorp/aws", version = "~> 6.0" } }
}
variable "cluster_arn" { type = string }
variable "oidc_provider_arn" { type = string }
locals {
  namespace  = "duckfleet-dev"
  bucket_arn = "arn:aws:s3:::duckfleet-dev-797781631727-us-east-2"
  state_arn  = "arn:aws:s3:::er-terraform-state-797781631727"
  tags       = { Project = "duckfleet", Environment = "dev", ManagedBy = "terraform" }
}
# Only this repository's immutable identity on main can obtain deployment credentials.
resource "aws_iam_role" "ci" {
  name                 = "duckfleet-ci-dev"
  max_session_duration = 7200
  tags                 = local.tags
  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect    = "Allow", Action = "sts:AssumeRoleWithWebIdentity"
      Principal = { Federated = var.oidc_provider_arn }
      Condition = { StringEquals = {
        "token.actions.githubusercontent.com:aud" = "sts.amazonaws.com"
        "token.actions.githubusercontent.com:sub" = "repo:athvin@273880343/duck-fleet@1452020329:ref:refs/heads/main"
      } }
    }]
  })
}
resource "aws_iam_role_policy" "ci" {
  name = "duckfleet-dev-deploy"
  role = aws_iam_role.ci.id
  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Sid = "DiscoverDev", Effect = "Allow", Action = ["eks:DescribeCluster"], Resource = [var.cluster_arn]
      },
      {
        Sid       = "ListOwnState", Effect = "Allow", Action = ["s3:ListBucket"], Resource = [local.state_arn]
        Condition = { StringLike = { "s3:prefix" = ["duckfleet/*"] } }
      },
      {
        Sid      = "OwnState", Effect = "Allow", Action = ["s3:GetObject", "s3:PutObject"]
        Resource = ["${local.state_arn}/duckfleet/dev.tfstate", "${local.state_arn}/duckfleet/dev.tfstate.tflock"]
      },
      {
        Sid      = "ReleaseOwnLock", Effect = "Allow", Action = ["s3:DeleteObject"]
        Resource = ["${local.state_arn}/duckfleet/dev.tfstate.tflock"]
      },
      {
        Sid = "OwnBucket", Effect = "Allow"
        Action = [
          "s3:CreateBucket", "s3:ListBucket", "s3:GetBucket*", "s3:GetEncryptionConfiguration",
          "s3:GetLifecycleConfiguration", "s3:GetReplicationConfiguration",
          "s3:GetAccelerateConfiguration", "s3:GetObjectLockConfiguration",
          "s3:PutBucketTagging", "s3:PutBucketPublicAccessBlock", "s3:PutBucketOwnershipControls",
          "s3:PutEncryptionConfiguration", "s3:PutBucketVersioning", "s3:PutBucketPolicy", "s3:PutLifecycleConfiguration"
        ]
        Resource = [local.bucket_arn]
      },
      {
        Sid = "RegistryLogin", Effect = "Allow", Action = ["ecr:GetAuthorizationToken"], Resource = ["*"]
      },
      {
        Sid = "OwnImages", Effect = "Allow"
        Action = [
          "ecr:CreateRepository", "ecr:DescribeRepositories", "ecr:ListTagsForResource", "ecr:TagResource", "ecr:UntagResource",
          "ecr:PutImageTagMutability", "ecr:PutImageScanningConfiguration", "ecr:DescribeImages", "ecr:ListImages",
          "ecr:BatchCheckLayerAvailability", "ecr:BatchGetImage", "ecr:GetDownloadUrlForLayer",
          "ecr:InitiateLayerUpload", "ecr:UploadLayerPart", "ecr:CompleteLayerUpload", "ecr:PutImage"
        ]
        Resource = [
          "arn:aws:ecr:us-east-2:797781631727:repository/duckfleet-dev/worker",
          "arn:aws:ecr:us-east-2:797781631727:repository/duckfleet-dev/dbt"
        ]
      }
    ]
  })
}
# No EKS managed access policy: Kubernetes RBAC grants only the DuckFleet namespace.
resource "aws_eks_access_entry" "ci" {
  cluster_name      = "er-dev"
  principal_arn     = aws_iam_role.ci.arn
  kubernetes_groups = ["duckfleet:deploy"]
  type              = "STANDARD"
  tags              = local.tags
}

# Platform owns the identity and association. DuckFleet CI has no IAM or EKS write permissions.
resource "aws_iam_role" "worker" {
  name = "duckfleet-dev-worker"
  tags = local.tags
  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect = "Allow", Principal = { Service = "pods.eks.amazonaws.com" }
      Action = ["sts:AssumeRole", "sts:TagSession"]
      Condition = { StringEquals = {
        "aws:RequestTag/eks-cluster-arn"            = var.cluster_arn
        "aws:RequestTag/kubernetes-namespace"       = local.namespace
        "aws:RequestTag/kubernetes-service-account" = "duckfleet-worker"
      } }
    }]
  })
}
resource "aws_iam_role_policy" "lake" {
  name = "duckfleet-lake-only"
  role = aws_iam_role.worker.id
  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Effect   = "Allow"
        Action   = ["s3:GetObject", "s3:PutObject", "s3:DeleteObject", "s3:AbortMultipartUpload", "s3:ListMultipartUploadParts"]
        Resource = ["${local.bucket_arn}/lake/*"]
      },
      {
        Effect    = "Allow", Action = ["s3:ListBucket"], Resource = [local.bucket_arn]
        Condition = { StringLike = { "s3:prefix" = ["lake/*"] } }
      },
      { Effect = "Allow", Action = ["s3:GetBucketLocation"], Resource = [local.bucket_arn] }
    ]
  })
}
resource "aws_eks_pod_identity_association" "worker" {
  cluster_name         = "er-dev"
  namespace            = local.namespace
  service_account      = "duckfleet-worker"
  role_arn             = aws_iam_role.worker.arn
  disable_session_tags = false
  tags                 = local.tags
}

