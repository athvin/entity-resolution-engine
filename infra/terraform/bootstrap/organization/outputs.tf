output "organization_id" {
  value = aws_organizations_organization.this.id
}

output "root_id" {
  value = aws_organizations_organization.this.roots[0].id
}

output "parked_nonprod_account_id" {
  value = aws_organizations_account.nonprod.id
}
