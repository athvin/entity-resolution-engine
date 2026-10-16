# Prod EKS (Auto Mode, per D1). Cluster admin is the ProdAdmin permission set —
# NOT PlatformAdmin, which the §8.2 guard denies eks:Delete*/Update* on
# er-prod-* clusters. The bootstrap creator (er-mgmt AdministratorAccess) is
# admin via bootstrap_cluster_creator_admin_permissions.
module "eks" {
  source = "../../modules/eks"

  name               = "er-prod"
  cluster_version    = "1.36"
  private_subnet_ids = module.network.private_subnet_ids

  admin_principal_arns = {
    prod_admin = "arn:aws:iam::797781631727:role/aws-reserved/sso.amazonaws.com/us-east-2/AWSReservedSSO_ProdAdmin_e1003e6485e33ff4"
  }
}

# metrics-server: EKS doesn't ship it; `kubectl top`, k9s, and HPAs need it.
resource "aws_eks_addon" "metrics_server" {
  cluster_name = module.eks.cluster_name
  addon_name   = "metrics-server"
}

output "cluster_name" {
  value = module.eks.cluster_name
}
