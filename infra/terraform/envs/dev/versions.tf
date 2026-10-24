terraform {
  required_version = ">= 1.11"

  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = "~> 6.0"
    }
    random = {
      source  = "hashicorp/random"
      version = "~> 3.0"
    }
    # §18.8 resolved the DNS plane to Cloudflare, and named this provider as
    # what writes into it: the wildcard dev certificate's ACM validation CNAME
    # and the SES DKIM / MAIL FROM records both land in the Cloudflare zone
    # this way. v5 is the current major; its record resource is
    # `cloudflare_dns_record` (v4's was `cloudflare_record`).
    cloudflare = {
      source  = "cloudflare/cloudflare"
      version = "~> 5.0"
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

# Credentials come from `CLOUDFLARE_API_TOKEN` in the environment, read by the
# provider natively. Deliberately not a Terraform variable: a variable would
# put the token in the plan file and in state, and this way the token is never
# written anywhere Terraform persists. The scoped token external-dns uses
# (Secrets Manager, §18.8) is the same kind of credential; export it before
# running a plan.
provider "cloudflare" {}
