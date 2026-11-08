# The §5 cluster, Auto Mode branch (D1, §5.5): no system node group, no
# Karpenter to operate — Auto Mode provisions from NodePool manifests (1.3),
# formats instance-store NVMe RAID0 on its own, enforces IMDSv2 hop-limit-1,
# and ships SOCI parallel pull. Custom NodePools (app/runner-sm/runner-l) are
# Kubernetes manifests applied after the cluster exists, not AWS API objects.

variable "name" {
  type = string
}

variable "cluster_version" {
  description = "Pinned here, not in the design doc (§5): newest minor at build time."
  type        = string
}

variable "private_subnet_ids" {
  type = list(string)
}

variable "admin_principal_arns" {
  description = "IAM principals granted cluster-admin via access entries."
  type        = map(string)
}

variable "viewer_principal_arns" {
  description = "IAM principals granted cluster-wide read via access entries (AmazonEKSViewPolicy). Read-only identities belong here, never in admin_principal_arns; the policy reads no Secret contents."
  type        = map(string)
  default     = {}
}

data "aws_iam_policy_document" "cluster_trust" {
  statement {
    effect  = "Allow"
    actions = ["sts:AssumeRole", "sts:TagSession"]

    principals {
      type        = "Service"
      identifiers = ["eks.amazonaws.com"]
    }
  }
}

resource "aws_iam_role" "cluster" {
  name               = "${var.name}-cluster"
  assume_role_policy = data.aws_iam_policy_document.cluster_trust.json
}

# The Auto Mode cluster-role set: cluster plus the compute/storage/LB/
# networking policies its managed controllers act under.
resource "aws_iam_role_policy_attachment" "cluster" {
  for_each = toset([
    "AmazonEKSClusterPolicy",
    "AmazonEKSComputePolicy",
    "AmazonEKSBlockStoragePolicy",
    "AmazonEKSLoadBalancingPolicy",
    "AmazonEKSNetworkingPolicy",
  ])

  role       = aws_iam_role.cluster.name
  policy_arn = "arn:aws:iam::aws:policy/${each.value}"
}

data "aws_iam_policy_document" "node_trust" {
  statement {
    effect  = "Allow"
    actions = ["sts:AssumeRole"]

    principals {
      type        = "Service"
      identifiers = ["ec2.amazonaws.com"]
    }
  }
}

resource "aws_iam_role" "node" {
  name               = "${var.name}-node"
  assume_role_policy = data.aws_iam_policy_document.node_trust.json
}

resource "aws_iam_role_policy_attachment" "node" {
  for_each = toset([
    "AmazonEKSWorkerNodeMinimalPolicy",
    "AmazonEC2ContainerRegistryPullOnly",
  ])

  role       = aws_iam_role.node.name
  policy_arn = "arn:aws:iam::aws:policy/${each.value}"
}

resource "aws_eks_cluster" "this" {
  name     = var.name
  version  = var.cluster_version
  role_arn = aws_iam_role.cluster.arn

  # API-based access entries, never the legacy aws-auth ConfigMap (§5).
  access_config {
    authentication_mode                         = "API"
    bootstrap_cluster_creator_admin_permissions = true
  }

  vpc_config {
    subnet_ids              = var.private_subnet_ids
    endpoint_public_access  = true
    endpoint_private_access = true
  }

  compute_config {
    enabled       = true
    node_pools    = ["system"]
    node_role_arn = aws_iam_role.node.arn
  }

  kubernetes_network_config {
    elastic_load_balancing {
      enabled = true
    }
  }

  storage_config {
    block_storage {
      enabled = true
    }
  }

  bootstrap_self_managed_addons = false

  depends_on = [aws_iam_role_policy_attachment.cluster]
}

resource "aws_eks_access_entry" "admin" {
  for_each = var.admin_principal_arns

  cluster_name  = aws_eks_cluster.this.name
  principal_arn = each.value
  type          = "STANDARD"
}

resource "aws_eks_access_policy_association" "admin" {
  for_each = var.admin_principal_arns

  cluster_name  = aws_eks_cluster.this.name
  principal_arn = each.value
  policy_arn    = "arn:aws:eks::aws:cluster-access-policy/AmazonEKSClusterAdminPolicy"

  access_scope {
    type = "cluster"
  }

  depends_on = [aws_eks_access_entry.admin]
}

resource "aws_eks_access_entry" "viewer" {
  for_each = var.viewer_principal_arns

  cluster_name  = aws_eks_cluster.this.name
  principal_arn = each.value
  type          = "STANDARD"
}

resource "aws_eks_access_policy_association" "viewer" {
  for_each = var.viewer_principal_arns

  cluster_name  = aws_eks_cluster.this.name
  principal_arn = each.value
  policy_arn    = "arn:aws:eks::aws:cluster-access-policy/AmazonEKSViewPolicy"

  access_scope {
    type = "cluster"
  }

  depends_on = [aws_eks_access_entry.viewer]
}

output "cluster_name" {
  value = aws_eks_cluster.this.name
}

# Needed to scope an `eks:DescribeCluster` grant to this cluster rather than to
# `*` — the CI deploy role in envs/dev/ci-deploy.tf is the first caller.
output "cluster_arn" {
  value = aws_eks_cluster.this.arn
}

output "cluster_endpoint" {
  value = aws_eks_cluster.this.endpoint
}

# The EKS-managed cluster security group, which Auto Mode attaches to the nodes.
#
# Needed to let an external load balancer reach pods. With an ALB target type of
# `ip` the targets are pod IPs, and a pod IP is a secondary address on a node
# ENI — so the traffic is evaluated against the NODE's security group, not
# against anything pod-shaped. envs/prod/edge.tf is the first caller.
#
# Read from the cluster rather than created here: Auto Mode owns this group's
# lifecycle, and declaring our own would leave two groups both claiming to
# govern node ingress.
output "cluster_security_group_id" {
  value = aws_eks_cluster.this.vpc_config[0].cluster_security_group_id
}

output "node_role_arn" {
  value = aws_iam_role.node.arn
}
