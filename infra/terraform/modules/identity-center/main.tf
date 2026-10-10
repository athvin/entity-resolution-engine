# The §8.1 permission-set model, rev 4 single-account shape: all three sets
# assign to the one account; environment separation is the er-prod-* prefix,
# enforced by the §8.2 guard denies that ride PlatformAdmin as inline policy.
# The console-created bootstrap pair (group `admins` + AdministratorAccess) is
# deliberately NOT managed here — bring-up credential, retired once CI applies.

data "aws_ssoadmin_instances" "this" {}

locals {
  instance_arn      = tolist(data.aws_ssoadmin_instances.this.arns)[0]
  identity_store_id = tolist(data.aws_ssoadmin_instances.this.identity_store_ids)[0]
}

variable "admin_username" {
  description = "Identity Center username of the (sole, for now) human operator."
  type        = string
}

variable "account_id" {
  description = "The single account every permission set assigns to."
  type        = string
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
  description       = "PlatformAdmin (guarded against er-prod-*) - infrastructure.md 8.1/8.2"
}

resource "aws_identitystore_group" "prod_admins" {
  identity_store_id = local.identity_store_id
  display_name      = "prod-admins"
  description       = "ProdAdmin - the only set without the 8.2 guard denies; deliberately one member"
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
  description      = "Full account except er-prod-* mutation (8.2 guard denies ride inline)"
  instance_arn     = local.instance_arn
  session_duration = "PT8H"
}

resource "aws_ssoadmin_managed_policy_attachment" "platform_admin" {
  instance_arn       = local.instance_arn
  permission_set_arn = aws_ssoadmin_permission_set.platform_admin.arn
  managed_policy_arn = "arn:aws:iam::aws:policy/AdministratorAccess"
}

# Rev 4: the two SCP statements become inline denies (infrastructure.md 8.2).
# An inline deny IS the identity-policy layer — removing it is one policy edit,
# which is why this file sits behind main's branch protection and why the 17
# probes re-run after any permission-set change.
resource "aws_ssoadmin_permission_set_inline_policy" "platform_admin_guard" {
  instance_arn       = local.instance_arn
  permission_set_arn = aws_ssoadmin_permission_set.platform_admin.arn

  inline_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Sid    = "DenyProdS3Mutation"
        Effect = "Deny"
        Action = ["s3:Put*", "s3:Delete*"]
        Resource = [
          "arn:aws:s3:::er-prod-*",
          "arn:aws:s3:::er-prod-*/*",
        ]
      },
      {
        Sid    = "DenyProdRdsMutation"
        Effect = "Deny"
        Action = ["rds:Delete*", "rds:Modify*"]
        Resource = [
          "arn:aws:rds:*:*:cluster:er-prod-*",
          "arn:aws:rds:*:*:db:er-prod-*",
        ]
      },
      {
        Sid    = "DenyProdEksMutation"
        Effect = "Deny"
        Action = ["eks:Delete*", "eks:Update*"]
        Resource = [
          "arn:aws:eks:*:*:cluster/er-prod-*",
          "arn:aws:eks:*:*:nodegroup/er-prod-*/*/*",
        ]
      },
      {
        Sid      = "DenyProdSecretsMutation"
        Effect   = "Deny"
        Action   = ["secretsmanager:Put*", "secretsmanager:Delete*"]
        Resource = "arn:aws:secretsmanager:*:*:secret:er/prod/*"
      },
      {
        # KMS keys are UUID-addressed, so the prod scope rides the Environment
        # tag that default_tags stamps on every prod-stack resource.
        Sid      = "DenyProdKeyDeletion"
        Effect   = "Deny"
        Action   = ["kms:ScheduleKeyDeletion"]
        Resource = "*"
        Condition = {
          StringEquals = { "aws:ResourceTag/Environment" = "prod" }
        }
      },
      {
        # Statement 2: being unable to mint er-prod-* roles is what makes the
        # er-prod-* application-role write path trustworthy (8.2).
        Sid    = "DenyProdRoleMinting"
        Effect = "Deny"
        Action = [
          "iam:CreateRole",
          "iam:PutRolePolicy",
          "iam:AttachRolePolicy",
          "iam:UpdateAssumeRolePolicy",
          "iam:DeleteRolePolicy",
          "iam:DetachRolePolicy",
        ]
        Resource = "arn:aws:iam::*:role/er-prod-*"
      },
      {
        # No long-lived credentials minted by humans, full stop (8.1).
        Sid      = "DenyAccessKeyCreation"
        Effect   = "Deny"
        Action   = ["iam:CreateAccessKey"]
        Resource = "arn:aws:iam::*:user/*"
      },
    ]
  })
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
  description      = "Full account, no guard denies, one-hour sessions (8.1) - assumption is alerted on (8.6)"
  instance_arn     = local.instance_arn
  session_duration = "PT1H"
}

resource "aws_ssoadmin_managed_policy_attachment" "prod_admin" {
  instance_arn       = local.instance_arn
  permission_set_arn = aws_ssoadmin_permission_set.prod_admin.arn
  managed_policy_arn = "arn:aws:iam::aws:policy/AdministratorAccess"
}

# --- Assignments (all to the one account) ----------------------------------

resource "aws_ssoadmin_account_assignment" "developers_platform" {
  instance_arn       = local.instance_arn
  permission_set_arn = aws_ssoadmin_permission_set.platform_admin.arn
  principal_id       = aws_identitystore_group.developers.group_id
  principal_type     = "GROUP"
  target_id          = var.account_id
  target_type        = "AWS_ACCOUNT"
}

resource "aws_ssoadmin_account_assignment" "developers_prod_read" {
  instance_arn       = local.instance_arn
  permission_set_arn = aws_ssoadmin_permission_set.prod_data_read_only.arn
  principal_id       = aws_identitystore_group.developers.group_id
  principal_type     = "GROUP"
  target_id          = var.account_id
  target_type        = "AWS_ACCOUNT"
}

resource "aws_ssoadmin_account_assignment" "prod_admins_prod" {
  instance_arn       = local.instance_arn
  permission_set_arn = aws_ssoadmin_permission_set.prod_admin.arn
  principal_id       = aws_identitystore_group.prod_admins.group_id
  principal_type     = "GROUP"
  target_id          = var.account_id
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
