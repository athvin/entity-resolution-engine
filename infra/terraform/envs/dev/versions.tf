terraform {
  required_version = ">= 1.11"

  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = "~> 6.0"
    }
  }
}

# The dev environment's workload stack (Phase 1). Same account as everything
# else (rev 4, infrastructure.md §3); separation is prefix and role.
provider "aws" {
  region = "us-east-2"

  default_tags {
    tags = {
      Project     = "entity-resolution"
      Environment = "dev"
      ManagedBy   = "terraform"
    }
  }
}
