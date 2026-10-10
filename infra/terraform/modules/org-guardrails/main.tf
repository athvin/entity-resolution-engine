# The two prod SCPs (infrastructure.md §8.2), attached to the prod OU so any
# account created inside it is guarded from birth. Deny-list model: the
# default FullAWSAccess policy stays attached; these carve out the denies.

variable "prod_ou_id" {
  description = "OU the two policies attach to."
  type        = string
}

variable "terraform_role_pattern" {
  description = "PrincipalArn pattern for the prod Terraform apply role (§14.2)."
  type        = string
  default     = "arn:aws:iam::*:role/er-terraform-apply"
}

locals {
  # Identity Center materializes ProdAdmin as AWSReservedSSO_ProdAdmin_<hash>
  # under the aws-reserved/sso.amazonaws.com path (region segment optional).
  prod_admin_pattern = "arn:aws:iam::*:role/aws-reserved/sso.amazonaws.com/*AWSReservedSSO_ProdAdmin_*"

  # Without BoolIfExists on PrincipalIsAWSService, the denies catch AWS's own
  # service-initiated writes — CloudTrail delivery, ALB access logs, ECR
  # replication — each failing with no obvious cause (§8.2).
  not_a_service = {
    test     = "BoolIfExists"
    variable = "aws:PrincipalIsAWSService"
    values   = ["false"]
  }
}

# Statement 1 — data mutation: only ProdAdmin, the platform's own er-prod-*
# roles, and Terraform may mutate prod data surfaces.
resource "aws_organizations_policy" "deny_prod_data_mutation" {
  name        = "DenyProdDataMutation"
  description = "Prod data surfaces are read-only except ProdAdmin, er-prod-* roles, and Terraform (infrastructure.md 8.2 statement 1)."
  type        = "SERVICE_CONTROL_POLICY"

  content = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Sid    = "DenyProdDataMutation"
      Effect = "Deny"
      Action = [
        "s3:Put*",
        "s3:Delete*",
        "rds:Delete*",
        "rds:Modify*",
        "eks:Delete*",
        "eks:Update*",
        "secretsmanager:Put*",
        "secretsmanager:Delete*",
        "kms:ScheduleKeyDeletion",
      ]
      Resource = "*"
      Condition = {
        ArnNotLike = {
          "aws:PrincipalArn" = [
            local.prod_admin_pattern,
            "arn:aws:iam::*:role/er-prod-*",
            var.terraform_role_pattern,
          ]
        }
        BoolIfExists = {
          "aws:PrincipalIsAWSService" = "false"
        }
      }
    }]
  })
}

# Statement 2 — privilege escalation: statement 1 exempts er-prod-*, so minting
# an er-prod-* role must itself be denied or the guardrail is a door (§8.2).
resource "aws_organizations_policy" "deny_prod_iam_escalation" {
  name        = "DenyProdIamEscalation"
  description = "No role minting or key creation in prod except ProdAdmin and Terraform (infrastructure.md 8.2 statement 2)."
  type        = "SERVICE_CONTROL_POLICY"

  content = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Sid    = "DenyProdIamEscalation"
      Effect = "Deny"
      Action = [
        "iam:CreateRole",
        "iam:PutRolePolicy",
        "iam:AttachRolePolicy",
        "iam:UpdateAssumeRolePolicy",
        "iam:CreateAccessKey",
      ]
      Resource = "*"
      Condition = {
        ArnNotLike = {
          "aws:PrincipalArn" = [
            local.prod_admin_pattern,
            var.terraform_role_pattern,
          ]
        }
        BoolIfExists = {
          "aws:PrincipalIsAWSService" = "false"
        }
      }
    }]
  })
}

resource "aws_organizations_policy_attachment" "data_mutation" {
  policy_id = aws_organizations_policy.deny_prod_data_mutation.id
  target_id = var.prod_ou_id
}

resource "aws_organizations_policy_attachment" "iam_escalation" {
  policy_id = aws_organizations_policy.deny_prod_iam_escalation.id
  target_id = var.prod_ou_id
}

output "policy_ids" {
  value = {
    data_mutation  = aws_organizations_policy.deny_prod_data_mutation.id
    iam_escalation = aws_organizations_policy.deny_prod_iam_escalation.id
  }
}
