output "organization_id" {
  value = aws_organizations_organization.this.id
}

output "root_id" {
  value = aws_organizations_organization.this.roots[0].id
}

output "nonprod_ou_id" {
  value = aws_organizations_organizational_unit.nonprod.id
}

output "prod_ou_id" {
  value = aws_organizations_organizational_unit.prod.id
}

output "nonprod_account_id" {
  value = aws_organizations_account.nonprod.id
}

output "prod_account_id" {
  value = aws_organizations_account.prod.id
}
