terraform {
  required_version = ">= 1.11"

  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = "~> 6.0"
    }
  }
}

# Rev 4 (infrastructure.md §3): one account. Runs with AWS_PROFILE=er-mgmt
# directly; the bucket lives in the account like everything else.
provider "aws" {
  region = "us-east-2"

  default_tags {
    tags = {
      Project     = "entity-resolution"
      Environment = "core"
      ManagedBy   = "terraform"
    }
  }
}
