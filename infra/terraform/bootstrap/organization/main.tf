# Bootstrap stack 1 of 2 (infrastructure.md §14): the Organization, OUs, and
# member accounts. Applied with local state first; the state migrates into the
# bucket that bootstrap/state-backend creates in nonprod (plan-to-aws.md 0.2).

# The organization predates Terraform: enabling IAM Identity Center auto-created
# it. This resource is imported (o-bqzb54i69b), never created fresh.
resource "aws_organizations_organization" "this" {
  feature_set = "ALL"

  # Identity Center enabled this principal when the org was created; omitting an
  # already-enabled principal here would disable it on apply.
  aws_service_access_principals = [
    "sso.amazonaws.com",
  ]

  # SCPs attach to the OUs below (infrastructure.md §8.2); the type must be
  # enabled on the root before any policy can attach.
  enabled_policy_types = [
    "SERVICE_CONTROL_POLICY",
  ]
}

resource "aws_organizations_organizational_unit" "nonprod" {
  name      = "nonprod"
  parent_id = aws_organizations_organization.this.roots[0].id
}

resource "aws_organizations_organizational_unit" "prod" {
  name      = "prod"
  parent_id = aws_organizations_organization.this.roots[0].id
}

resource "aws_organizations_account" "nonprod" {
  name      = "nonprod"
  email     = var.nonprod_root_email
  parent_id = aws_organizations_organizational_unit.nonprod.id

  role_name                  = "OrganizationAccountAccessRole"
  iam_user_access_to_billing = "ALLOW"
  close_on_deletion          = false

  lifecycle {
    prevent_destroy = true
    # role_name is create-only and cannot be read back from the API.
    ignore_changes = [role_name]
  }
}

resource "aws_organizations_account" "prod" {
  name      = "prod"
  email     = var.prod_root_email
  parent_id = aws_organizations_organizational_unit.prod.id

  role_name                  = "OrganizationAccountAccessRole"
  iam_user_access_to_billing = "ALLOW"
  close_on_deletion          = false

  lifecycle {
    prevent_destroy = true
    ignore_changes  = [role_name]
  }
}
