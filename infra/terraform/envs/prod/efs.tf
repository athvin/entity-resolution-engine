# Prod EFS (config roots + CSV import drops) + the EFS CSI driver addon, the one
# storage addon Auto Mode does not bundle; its controller authenticates via EKS
# Pod Identity. Mirrors envs/dev/efs.tf with er-prod-* names.
module "efs" {
  source = "../../modules/efs"

  name       = "er-prod-config"
  vpc_id     = module.network.vpc_id
  vpc_cidr   = "10.1.0.0/16"
  subnet_ids = module.network.private_subnet_ids
}

# The one prod namespace's access point. Dev's are created by
# `scripts/dev_env.sh up` (a namespace per developer, so Terraform cannot know
# the list); prod has exactly one namespace and no dev_env.sh, so the access
# point is Terraform's here — the same argument secrets.tf makes for the §9
# entries. Nothing else creates it: verified 2026-11-08 that the file system
# had zero access points nine days after the cluster existed, which is the
# failure a prod deploy would have hit at the first mount.
#
# The shape mirrors dev_env.sh's `create-access-point` exactly — posix
# 1001:1001 (the er uid in docker/Dockerfile.api), 0750 creation — so the
# chart's `efs.accessPointId` means the same thing in both environments. The
# deploy discovers it by root path (`/app`), the same query dev_env.sh runs;
# the output below is for humans, not for the pipeline.
resource "aws_efs_access_point" "app" {
  file_system_id = module.efs.file_system_id

  posix_user {
    uid = 1001
    gid = 1001
  }

  root_directory {
    path = "/app"

    creation_info {
      owner_uid   = 1001
      owner_gid   = 1001
      permissions = "0750"
    }
  }

  tags = {
    Name = "er-prod-config-app"
  }
}

output "app_efs_access_point_id" {
  description = "The /app access point the prod er-platform release mounts (efs.accessPointId)."
  value       = aws_efs_access_point.app.id
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
  name               = "er-prod-efs-csi"
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
