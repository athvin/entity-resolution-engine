# State lives in the account's state bucket (infrastructure.md §14).
terraform {
  backend "s3" {
    bucket       = "er-terraform-state-797781631727"
    key          = "bootstrap/organization.tfstate"
    region       = "us-east-2"
    encrypt      = true
    use_lockfile = true
  }
}
