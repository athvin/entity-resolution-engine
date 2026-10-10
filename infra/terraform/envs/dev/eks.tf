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

output "cluster_name" {
  value = module.eks.cluster_name
}
