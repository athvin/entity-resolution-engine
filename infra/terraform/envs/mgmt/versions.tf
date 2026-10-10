terraform {
  required_version = ">= 1.11"

  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = "~> 6.0"
    }
  }
}

# Management account (run with AWS_PROFILE=er-mgmt). No workloads live here —
# only billing, audit, and org-level resources (infrastructure.md §3).
provider "aws" {
  region = "us-east-2"

  default_tags {
    tags = {
      Project     = "entity-resolution"
      Environment = "mgmt"
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
      Environment = "mgmt"
      ManagedBy   = "terraform"
    }
  }
}
