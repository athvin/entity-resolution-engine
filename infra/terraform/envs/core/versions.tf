terraform {
  required_version = ">= 1.11"

  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = "~> 6.0"
    }
    # The sccache bucket moved to R2 (#909, see r2.tf for why neither S3 nor the
    # Actions cache backend worked). `~> 5.0` matches the pin `envs/dev` already
    # carries, so the two envs cannot drift onto different majors — v5 renamed
    # several resources from v4 (`cloudflare_record` -> `cloudflare_dns_record`),
    # which is exactly the kind of break a split pin would hide.
    cloudflare = {
      source  = "cloudflare/cloudflare"
      version = "~> 5.0"
    }
  }
}

# Credentials come from `CLOUDFLARE_API_TOKEN` in the environment, read by the
# provider natively — the same choice `envs/dev` makes, for the same reason: a
# Terraform variable would put the token in the plan file and in state, and this
# way it is never written anywhere Terraform persists.
#
# With `var.cloudflare_account_id` unset, r2.tf instantiates nothing and no plan
# needs this credential at all.
provider "cloudflare" {}

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
