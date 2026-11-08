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
    # The DNS plane (§18.8). Same major as envs/dev — v5, whose record resource
    # is `cloudflare_dns_record` (v4's was `cloudflare_record`).
    cloudflare = {
      source  = "cloudflare/cloudflare"
      version = "~> 5.0"
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

# Credentials come from `CLOUDFLARE_API_TOKEN` in the environment, read by the
# provider natively — the same convention envs/dev/versions.tf uses, and for the
# reason stated there: a Terraform VARIABLE would put the token in the plan file
# and in state, and the apply tier plans to `tfplan` and applies that file. This
# way the token is never written anywhere Terraform persists.
#
# Only the apply tier is given it (scripts/ci/tf-cloudflare-token.sh, prod only).
# The plan tier runs credential-free and stays that way: a `-refresh=false` plan
# over Cloudflare resources needs no token, which edge.tf's header records as a
# measured result along with the constraint it implies — no Cloudflare `data`
# sources in this stack.
provider "cloudflare" {}
