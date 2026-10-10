terraform {
  backend "s3" {
    bucket       = "er-terraform-state-797781631727"
    key          = "envs/core.tfstate"
    region       = "us-east-2"
    encrypt      = true
    use_lockfile = true
  }
}
