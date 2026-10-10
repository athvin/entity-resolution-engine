terraform {
  required_version = ">= 1.11"

  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = "~> 6.0"
    }
  }
}

# Nonprod workload account. Run with AWS_PROFILE=er-mgmt; the provider assumes
# the org access role (PlatformAdmin via SSO becomes the human path; CI OIDC
# the automated one).
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
