terraform {
  required_version = ">= 1.11"

  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = "~> 6.0"
    }
  }
}

# Runs with management-account credentials (AWS_PROFILE=er-mgmt) and assumes
# into nonprod: the state bucket lives there (infrastructure.md §14), and this
# stack cannot use that bucket as its own backend until after first apply.
provider "aws" {
  region = "us-east-2"

  assume_role {
    role_arn = "arn:aws:iam::660360495170:role/OrganizationAccountAccessRole"
  }

  default_tags {
    tags = {
      Project     = "entity-resolution"
      Environment = "nonprod"
      ManagedBy   = "terraform"
    }
  }
}
