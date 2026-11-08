terraform {
  required_version = ">= 1.11"

  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = "~> 6.0"
    }
    random = {
      # >= 3.6 for `random_bytes`, which secrets.tf uses for the two values
      # whose encoding is load-bearing (hex tokens, and the base64 sealing key
      # frontend/src/lib/env.ts validates).
      source  = "hashicorp/random"
      version = "~> 3.6"
    }
  }
}

# The prod workload stack (Phase 3). Same account as dev (rev 4,
# infrastructure.md §3); separation is the er-prod-* name prefix + the §8.2
# guard denies carried by PlatformAdmin (and the builder key), PLUS the
# Environment=prod tag the §8.2 KMS prod-deny and the ProdAdmin alerting key on.
# default_tags stamping Environment=prod on every resource here is therefore
# load-bearing, not cosmetic. Apply under AWS_PROFILE=er-mgmt (bootstrap admin)
# or ProdAdmin — PlatformAdmin is denied er-prod-* mutation by design.
provider "aws" {
  region = "us-east-2"

  default_tags {
    tags = {
      Project     = "entity-resolution"
      Environment = "prod"
      ManagedBy   = "terraform"
    }
  }
}
