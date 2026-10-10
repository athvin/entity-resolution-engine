# The §8.1 permission-set model: access by group membership, never per-person.
# The console-created bootstrap pair (group `admins` + AdministratorAccess on
# the management account) is deliberately NOT managed here — it is the
# bring-up credential, slated for retirement once CI applies Terraform (§14.2);
# everything designed lives in this module so access review is `git log`.

data "aws_ssoadmin_instances" "this" {}

locals {
  instance_arn      = tolist(data.aws_ssoadmin_instances.this.arns)[0]
  identity_store_id = tolist(data.aws_ssoadmin_instances.this.identity_store_ids)[0]
}

variable "admin_username" {
  description = "Identity Center username of the (sole, for now) human operator."
  type        = string
}

variable "nonprod_account_id" {
  type = string
}

variable "prod_account_id" {
  description = "Set once the prod account exists; null defers the prod assignments."
  type        = string
  default     = null
}

data "aws_identitystore_user" "admin" {
  identity_store_id = local.identity_store_id

  alternate_identifier {
    unique_attribute {
      attribute_path  = "UserName"
      attribute_value = var.admin_username
    }
  }
}

# --- Groups ---------------------------------------------------------------

resource "aws_identitystore_group" "developers" {
  identity_store_id = local.identity_store_id
  display_name      = "developers"
  description       = "PlatformAdmin in nonprod, ProdDataReadOnly in prod (infrastructure.md 8.1)"
}

resource "aws_identitystore_group" "prod_admins" {
  identity_store_id = local.identity_store_id
  display_name      = "prod-admins"
  description       = "ProdAdmin in prod - deliberately one member (infrastructure.md 8.1)"
}

resource "aws_identitystore_group_membership" "admin_developer" {
  identity_store_id = local.identity_store_id
  group_id          = aws_identitystore_group.developers.group_id
  member_id         = data.aws_identitystore_user.admin.user_id
}

resource "aws_identitystore_group_membership" "admin_prod_admin" {
  identity_store_id = local.identity_store_id
  group_id          = aws_identitystore_group.prod_admins.group_id
  member_id         = data.aws_identitystore_user.admin.user_id
}

# --- Permission sets -------------------------------------------------------

resource "aws_ssoadmin_permission_set" "platform_admin" {
  name             = "PlatformAdmin"
  description      = "Full nonprod - mutating dev is unrestricted by design (8.1)"
  instance_arn     = local.instance_arn
  session_duration = "PT8H"
}

resource "aws_ssoadmin_managed_policy_attachment" "platform_admin" {
  instance_arn       = local.instance_arn
  permission_set_arn = aws_ssoadmin_permission_set.platform_admin.arn
  managed_policy_arn = "arn:aws:iam::aws:policy/AdministratorAccess"
}

resource "aws_ssoadmin_permission_set" "prod_data_read_only" {
  name             = "ProdDataReadOnly"
  description      = "Read the prod lake, connect as er_readonly, view-only EKS, logs read (8.1)"
  instance_arn     = local.instance_arn
  session_duration = "PT4H"
}

resource "aws_ssoadmin_permission_set_inline_policy" "prod_data_read_only" {
  instance_arn       = local.instance_arn
  permission_set_arn = aws_ssoadmin_permission_set.prod_data_read_only.arn

  inline_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Sid    = "ProdLakeRead"
        Effect = "Allow"
        Action = ["s3:GetObject", "s3:ListBucket"]
        Resource = [
          "arn:aws:s3:::er-prod-lake",
          "arn:aws:s3:::er-prod-lake/*",
        ]
      },
      {
        Sid      = "LakeKeyDecryptViaS3"
        Effect   = "Allow"
        Action   = ["kms:Decrypt"]
        Resource = "*"
        Condition = {
          StringEquals = {
            "kms:ViaService" = "s3.us-east-2.amazonaws.com"
          }
        }
      },
      {
        Sid      = "ReadonlyDbConnect"
        Effect   = "Allow"
        Action   = ["rds-db:connect"]
        Resource = "arn:aws:rds-db:us-east-2:*:dbuser:*/er_readonly"
      },
      {
        Sid      = "DescribeEks"
        Effect   = "Allow"
        Action   = ["eks:DescribeCluster", "eks:ListClusters"]
        Resource = "*"
      },
      {
        # PII path by design; safe only under 10.4's identifiers-and-counts
        # log rule, which 17 verifies behaviourally.
        Sid    = "LogsRead"
        Effect = "Allow"
        Action = [
          "logs:DescribeLogGroups",
          "logs:DescribeLogStreams",
          "logs:GetLogEvents",
          "logs:FilterLogEvents",
          "logs:StartQuery",
          "logs:GetQueryResults",
        ]
        Resource = "*"
      },
    ]
  })
}

resource "aws_ssoadmin_permission_set" "prod_admin" {
  name             = "ProdAdmin"
  description      = "Full prod, one-hour sessions, prod-admins group only (8.1)"
  instance_arn     = local.instance_arn
  session_duration = "PT1H"
}

resource "aws_ssoadmin_managed_policy_attachment" "prod_admin" {
  instance_arn       = local.instance_arn
  permission_set_arn = aws_ssoadmin_permission_set.prod_admin.arn
  managed_policy_arn = "arn:aws:iam::aws:policy/AdministratorAccess"
}

# --- Assignments -----------------------------------------------------------

resource "aws_ssoadmin_account_assignment" "developers_nonprod" {
  instance_arn       = local.instance_arn
  permission_set_arn = aws_ssoadmin_permission_set.platform_admin.arn
  principal_id       = aws_identitystore_group.developers.group_id
  principal_type     = "GROUP"
  target_id          = var.nonprod_account_id
  target_type        = "AWS_ACCOUNT"
}

resource "aws_ssoadmin_account_assignment" "developers_prod_read" {
  count = var.prod_account_id == null ? 0 : 1

  instance_arn       = local.instance_arn
  permission_set_arn = aws_ssoadmin_permission_set.prod_data_read_only.arn
  principal_id       = aws_identitystore_group.developers.group_id
  principal_type     = "GROUP"
  target_id          = var.prod_account_id
  target_type        = "AWS_ACCOUNT"
}

resource "aws_ssoadmin_account_assignment" "prod_admins_prod" {
  count = var.prod_account_id == null ? 0 : 1

  instance_arn       = local.instance_arn
  permission_set_arn = aws_ssoadmin_permission_set.prod_admin.arn
  principal_id       = aws_identitystore_group.prod_admins.group_id
  principal_type     = "GROUP"
  target_id          = var.prod_account_id
  target_type        = "AWS_ACCOUNT"
}

output "permission_set_arns" {
  value = {
    platform_admin      = aws_ssoadmin_permission_set.platform_admin.arn
    prod_data_read_only = aws_ssoadmin_permission_set.prod_data_read_only.arn
    prod_admin          = aws_ssoadmin_permission_set.prod_admin.arn
  }
}

output "group_ids" {
  value = {
    developers  = aws_identitystore_group.developers.group_id
    prod_admins = aws_identitystore_group.prod_admins.group_id
  }
}
