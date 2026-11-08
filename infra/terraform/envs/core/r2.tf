# The sccache compilation cache on Cloudflare R2, replacing the S3 bucket in
# sccache.tf. #909.
#
# WHY R2 AND NOT EITHER OF THE TWO BACKENDS WE HAVE ALREADY TRIED.
#
# S3 (#350) was fast and expensive. GitHub-hosted runners sit outside AWS, so
# every cache read is billed internet egress at ~$0.09/GB and no VPC endpoint
# can reach it — the far end is not in the VPC. Measured by CUR 2.0
# per-resource attribution for November: `er-ci-sccache` /
# `DataTransfer-Out-Bytes` was $209.32 of a $211.62 S3 total, 2,425.7 GB, while
# storage was $0.02. The next-largest S3 line anywhere in the account was $0.05.
#
# The Actions cache backend (#893) was cheap and slow. Two measured causes:
# per-hit read latency roughly doubled (`Average cache read hit` 0.122s on S3
# against 0.242s on ghac), and — dominant — GitHub rate-limits cache writes
# PER WORKFLOW RUN, shared across every job in it, while sccache's
# `GHACache::build` installs no `RetryLayer` and so counts the resulting HTTP
# 429 instead of retrying it (mozilla/sccache#2821, unfixed in the pinned
# 0.19.0). The first `main` run dropped 3,763 writes and landed 1,164 — a 24%
# success rate, hit rates averaging ~27%, and 25.1 minutes of wall against 9.8
# on S3.
#
# R2 is the only option that is both. Zero egress fees, and because it is not
# GitHub's cache service neither the 429 bug nor the per-run write quota
# applies. At this footprint: ~4.8 GB at $0.015/GB-mo = ~$0.07, plus ~4.6M
# Class B reads at $0.36/M = ~$1.66. Call it $2/month against $209.
#
# WHAT R2 COSTS US, stated plainly because it is the one way this is a step
# backwards: the Actions backend gave cache-poisoning protection for free via
# branch scoping — verified on #893, where all 1,137 entries landed on
# `refs/pull/893/merge` and none on `refs/heads/main`. R2 has no OIDC and no IAM
# condition keys, so that property is rebuilt out of two scoped API tokens and a
# `main`-only GitHub environment. See `rust.yml` for that half.

# --- opt-in, so the other resources in this env stay plannable ---------------
#
# Exactly the shape `envs/dev/ses.tf` uses, and for exactly its reason. Without
# a `CLOUDFLARE_API_TOKEN` the provider cannot authenticate, and once
# `terraform plan` became a CI gate (§14.2, PR #352) that meant one credential
# the repo does not hold could fail the WHOLE env's plan — leaving every
# resource that does plan cleanly unchecked. With `cloudflare_account_id` null
# this file contributes nothing.
#
# Unlike ses.tf this is meant to stop being opt-in: #908 puts a scoped
# read-only Cloudflare token in CI so plans authenticate, and the gate is worth
# more here than in ses.tf because this bucket is on the critical path of every
# compile job. Remove the `count`s when #908 lands.
locals {
  r2_enabled = var.cloudflare_account_id != null

  # Account-scoped, so unlike `er-ci-sccache-${local.mgmt_account_id}` in
  # sccache.tf there is no global-uniqueness suffix to carry. R2 bucket names
  # are unique within an account only.
  r2_sccache_bucket = "er-ci-sccache"
}

resource "cloudflare_r2_bucket" "sccache" {
  count = local.r2_enabled ? 1 : 0

  account_id = var.cloudflare_account_id
  name       = local.r2_sccache_bucket

  # `enam` — eastern North America. The consumers are GitHub-hosted runners,
  # which are predominantly US East, and the bucket this replaces was us-east-2.
  #
  # Note the provider's own caveat: location is honoured only the FIRST time a
  # bucket with a given name is created, and is best-effort rather than a
  # guarantee. Deleting and recreating under the same name keeps the original
  # location, so this cannot be corrected by a taint.
  location = "enam"

  # Standard, not InfrequentAccess. IA bills retrieval, and every object here is
  # read far more often than it is written — the whole point of the cache. The
  # bucket is ~4.8 GB, so there is no storage saving worth a retrieval charge.
  storage_class = "Standard"
}

# Matches sccache.tf's expiry exactly, because the reasoning transfers: 14 days
# is long enough to survive a quiet fortnight on a dependency that has not
# changed, and short enough that the bucket does not grow without bound.
#
# NB `max_age` is in SECONDS here, not days as in the S3 lifecycle rule.
#
# TWO ONE-WAY DOORS on this resource, both reported by the provider rather than
# guessed at. The first plan against a real account warned:
#
#   Warning: Resource Destruction Considerations
#   This resource cannot be destroyed from Terraform. If you create this
#   resource, it will be present in the API until manually deleted.
#
# and the provider docs add that it does not support `terraform import`. So a
# `terraform destroy` of this env leaves the rule behind, and a rule that drifts
# cannot be adopted back into state — it has to be deleted in the dashboard and
# recreated. Neither is a reason not to have an expiry rule; both are reasons
# not to iterate on this resource casually.
resource "cloudflare_r2_bucket_lifecycle" "sccache" {
  count = local.r2_enabled ? 1 : 0

  account_id  = var.cloudflare_account_id
  bucket_name = cloudflare_r2_bucket.sccache[0].name

  rules = [{
    id      = "expire-cache-objects"
    enabled = true

    # Empty prefix scopes the rule to every object, which is what we want: every
    # key in this bucket is a content-addressed compilation result.
    conditions = {
      prefix = ""
    }

    delete_objects_transition = {
      condition = {
        type    = "Age"
        max_age = 14 * 24 * 60 * 60 # 14 days
      }
    }

    # sccache writes whole objects, but a killed runner can leave a multipart
    # upload behind, and those are billed until cleaned up.
    abort_multipart_uploads_transition = {
      condition = {
        type    = "Age"
        max_age = 24 * 60 * 60 # 1 day
      }
    }
  }]
}

variable "cloudflare_account_id" {
  description = <<-EOT
    The Cloudflare account that owns the R2 bucket. Visible in any dashboard
    URL (`dash.cloudflare.com/<account-id>/...`) and not a secret — the S3-API
    endpoint is derived from it, so it appears in `rust.yml` in the clear.

    NULL (the default) disables this whole file — see `local.r2_enabled`. The
    credential itself is never a variable: the provider reads
    `CLOUDFLARE_API_TOKEN` from the environment, so the token is not written
    into the plan file or into state.
  EOT
  type        = string
  default     = null
}

output "r2_sccache_endpoint" {
  description = <<-EOT
    The S3-API endpoint sccache talks to, as `SCCACHE_ENDPOINT`. Emitted rather
    than hand-written into the workflow so the account id has one home.

    Pair it with `SCCACHE_REGION=auto`: R2 has no regions, and the S3 client
    requires the field to be set to something.
  EOT
  value       = local.r2_enabled ? "https://${var.cloudflare_account_id}.r2.cloudflarestorage.com" : null
}

output "r2_sccache_bucket" {
  description = "Bucket name, as `SCCACHE_BUCKET`."
  value       = local.r2_enabled ? local.r2_sccache_bucket : null
}
