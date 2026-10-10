# The bucket this stack itself created; state migrated in after first apply.
terraform {
  backend "s3" {
    bucket       = "er-terraform-state-797781631727"
    key          = "bootstrap/state-backend.tfstate"
    region       = "us-east-2"
    encrypt      = true
    use_lockfile = true
  }
}
