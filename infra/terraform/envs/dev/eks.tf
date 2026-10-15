# Task 1.2 (§5, Auto Mode per D1).
module "eks" {
  source = "../../modules/eks"

  name               = "er-dev"
  cluster_version    = "1.36"
  private_subnet_ids = module.network.private_subnet_ids

  admin_principal_arns = {
    # The PlatformAdmin permission set's materialized role (§8.1); the
    # bootstrap creator (er-mgmt AdministratorAccess) is covered by
    # bootstrap_cluster_creator_admin_permissions.
    platform_admin = "arn:aws:iam::797781631727:role/aws-reserved/sso.amazonaws.com/us-east-2/AWSReservedSSO_PlatformAdmin_a9350569d9b23f7f"
  }
}

# metrics-server: EKS doesn't ship it, so `kubectl top`, k9s CPU/MEM, and HPAs
# have no metrics source without it. The AWS-managed add-on keeps the manifests
# patched in kube-system. (Adopt the already-created add-on into state with:
#   terraform import aws_eks_addon.metrics_server er-dev:metrics-server )
resource "aws_eks_addon" "metrics_server" {
  cluster_name = module.eks.cluster_name
  addon_name   = "metrics-server"
}

output "cluster_name" {
  value = module.eks.cluster_name
}
