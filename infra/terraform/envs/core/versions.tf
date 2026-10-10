terraform {
  required_version = ">= 1.11"

  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = "~> 6.0"
    }
  }
}

# The account (rev 4, infrastructure.md §3). Run with AWS_PROFILE=er-mgmt.
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

# Billing-adjacent APIs (Data Exports, Budgets) are served from us-east-1.
provider "aws" {
  alias  = "use1"
  region = "us-east-1"

  default_tags {
    tags = {
      Project     = "entity-resolution"
      Environment = "core"
      ManagedBy   = "terraform"
    }
  }
}
