# Task 1.5 (§7.4). The EFS CSI driver is the one storage addon Auto Mode does
# not bundle; its controller authenticates via EKS Pod Identity.
module "efs" {
  source = "../../modules/efs"

  name       = "er-dev-config"
  vpc_id     = module.network.vpc_id
  vpc_cidr   = "10.0.0.0/16"
  subnet_ids = module.network.private_subnet_ids
}

data "aws_iam_policy_document" "efs_csi_trust" {
  statement {
    effect  = "Allow"
    actions = ["sts:AssumeRole", "sts:TagSession"]

    principals {
      type        = "Service"
      identifiers = ["pods.eks.amazonaws.com"]
    }
  }
}

resource "aws_iam_role" "efs_csi" {
  name               = "er-dev-efs-csi"
  assume_role_policy = data.aws_iam_policy_document.efs_csi_trust.json
}

resource "aws_iam_role_policy_attachment" "efs_csi" {
  role       = aws_iam_role.efs_csi.name
  policy_arn = "arn:aws:iam::aws:policy/service-role/AmazonEFSCSIDriverPolicy"
}

resource "aws_eks_addon" "efs_csi" {
  cluster_name                = module.eks.cluster_name
  addon_name                  = "aws-efs-csi-driver"
  resolve_conflicts_on_update = "OVERWRITE"

  pod_identity_association {
    role_arn        = aws_iam_role.efs_csi.arn
    service_account = "efs-csi-controller-sa"
  }
}

output "efs_file_system_id" {
  value = module.efs.file_system_id
}
