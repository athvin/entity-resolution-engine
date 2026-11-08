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

    # The CI deploy role (#556's prod deploy path). Same shape as dev's
    # ci_deploy entry: this module's access entries associate
    # AmazonEKSClusterAdminPolicy, which is more than a helm upgrade strictly
    # needs. Prod keeps the wide grant for two reasons dev did not have: the
    # same role applies the cluster-scoped objects (#927 — NodePools and
    # PriorityClasses cannot be namespace-scoped), and the TargetGroupBinding
    # controller contract needs the object read across namespaces. The trust
    # policy stays pinned to pushes on `main` (ci-deploy.tf).
    ci_deploy = aws_iam_role.ci_deploy_prod.arn
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
