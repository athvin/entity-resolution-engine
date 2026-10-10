# Bootstrap stack 1 of 2 (infrastructure.md §14, rev 4): the Organization —
# kept only because Identity Center requires an organization instance — and
# the parked former-nonprod member account. The rev 1-3 OUs, SCPs, and prod
# account were removed when the operator collapsed to a single account (§3);
# git history holds the three-account shape.

# The organization predates Terraform: enabling IAM Identity Center auto-created
# it. This resource is imported (o-bqzb54i69b), never created fresh.
resource "aws_organizations_organization" "this" {
  feature_set = "ALL"

  # Identity Center enabled this principal when the org was created; omitting an
  # already-enabled principal here would disable it on apply.
  aws_service_access_principals = [
    "sso.amazonaws.com",
  ]

  # SERVICE_CONTROL_POLICY stays enabled on the root even though no custom SCP
  # exists after rev 4 — disabling a policy type detaches policies org-wide and
  # re-enabling is churn for nothing if the account split returns.
  enabled_policy_types = [
    "SERVICE_CONTROL_POLICY",
  ]
}

# Parked, empty, at the org root (§3): zero cost, kept in case a future team
# size re-justifies the account split. Close it instead if that optionality
# stops being worth the list-entry.
resource "aws_organizations_account" "nonprod" {
  name      = "nonprod"
  email     = var.nonprod_root_email
  parent_id = aws_organizations_organization.this.roots[0].id

  role_name                  = "OrganizationAccountAccessRole"
  iam_user_access_to_billing = "ALLOW"
  close_on_deletion          = false

  lifecycle {
    prevent_destroy = true
    # role_name is create-only and cannot be read back from the API.
    ignore_changes = [role_name]
  }
}
