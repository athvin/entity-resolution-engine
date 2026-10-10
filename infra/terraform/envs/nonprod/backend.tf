terraform {
  backend "s3" {
    bucket       = "er-terraform-state-660360495170"
    key          = "envs/nonprod.tfstate"
    region       = "us-east-2"
    encrypt      = true
    use_lockfile = true

    assume_role = {
      role_arn = "arn:aws:iam::660360495170:role/OrganizationAccountAccessRole"
    }
  }
}
