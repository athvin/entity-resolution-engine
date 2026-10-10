# The bucket this stack itself created; state migrated in after first apply.
terraform {
  backend "s3" {
    bucket       = "er-terraform-state-660360495170"
    key          = "bootstrap/state-backend.tfstate"
    region       = "us-east-2"
    encrypt      = true
    use_lockfile = true

    assume_role = {
      role_arn = "arn:aws:iam::660360495170:role/OrganizationAccountAccessRole"
    }
  }
}
