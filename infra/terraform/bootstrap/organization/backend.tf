# State lives in the nonprod bucket (infrastructure.md §14) while the stack's
# provider targets the management account — backend auth is independent.
terraform {
  backend "s3" {
    bucket       = "er-terraform-state-660360495170"
    key          = "bootstrap/organization.tfstate"
    region       = "us-east-2"
    encrypt      = true
    use_lockfile = true

    assume_role = {
      role_arn = "arn:aws:iam::660360495170:role/OrganizationAccountAccessRole"
    }
  }
}
