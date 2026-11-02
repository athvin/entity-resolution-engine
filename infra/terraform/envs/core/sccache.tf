# The sccache compilation cache, moved off the GitHub Actions cache backend.
#
# WHY: the Actions backend shares one 10 GB per-repository budget, LRU-evicted
# past it, which is why `.github/workflows/rust.yml` carries a nightly
# `cache-prune` job — a cap on the thing doing most of the work. When the
# repository sat at the ceiling, GitHub evicted its largest LRU victim (the
# rust-cache entry every compile job restores from) and produced cold runs with
# no dependency change to explain them. Cold is roughly a 3x penalty: on
# DuckDB 2.0 the `DuckLake + extensions e2e` job measured 25m cold against 8m
# warm, and `test` 15m against 6m.
#
# S3 has no such budget, so the prune job and the eviction class both go away.
# This is the dominant remaining CI lever: after collapsing 147 test binaries to
# 56 (PR #345), what is left in both long jobs is dependency-tree compile, which
# only a cache can touch.
#
# APPLIED 2026-10-28/29 (manual apply between PR #346 and PR #350, exactly the
# sequence the original note here prescribed: merge the terraform, apply, then
# point the workflow at the bucket). `.github/workflows/rust.yml` has been on
# this backend since #350; the read-only PR role's denied writes surface as
# counted misses, not failures — verified in production, see below.

# --- the bucket ---------------------------------------------------------------

resource "aws_s3_bucket" "sccache" {
  # `local.mgmt_account_id` is declared in audit.tf; every .tf in this directory
  # shares one module scope, and suffixing the account id is the convention
  # `er-audit-archive-${local.mgmt_account_id}` already follows.
  bucket = "er-ci-sccache-${local.mgmt_account_id}"
}

# Private, and belt-and-braces about it: the bucket holds compiled object code
# for this repository, which is not something to serve anonymously whatever the
# repository's own visibility happens to be.
resource "aws_s3_bucket_public_access_block" "sccache" {
  bucket                  = aws_s3_bucket.sccache.id
  block_public_acls       = true
  block_public_policy     = true
  ignore_public_acls      = true
  restrict_public_buckets = true
}

resource "aws_s3_bucket_server_side_encryption_configuration" "sccache" {
  bucket = aws_s3_bucket.sccache.id

  rule {
    apply_server_side_encryption_by_default {
      sse_algorithm = "AES256"
    }
  }
}

# Deliberately NOT versioned. Every object is a content-addressed compilation
# result — a second version of the same key is the same bytes — so versioning
# would only accumulate cost and make expiry harder to reason about.
resource "aws_s3_bucket_lifecycle_configuration" "sccache" {
  bucket = aws_s3_bucket.sccache.id

  rule {
    id     = "expire-cache-objects"
    status = "Enabled"

    filter {}

    # 14 days: long enough to survive a quiet fortnight on a dependency that
    # has not changed, short enough that the bucket does not grow without
    # bound. This replaces the nightly prune, which existed only because the
    # Actions backend had a hard ceiling.
    expiration {
      days = 14
    }

    # sccache writes whole objects, but a killed runner can leave a multipart
    # upload behind, and those are billed until cleaned up.
    abort_incomplete_multipart_upload {
      days_after_initiation = 1
    }
  }
}

# --- who may write, and who may only read ------------------------------------

# TWO roles, and the split is the security decision in this file.
#
# A role that let any PR-triggered job write to the cache would be a path for a
# branch to place compiled object code that a later `main` build consumes —
# cache poisoning straight into the build. The repository is PRIVATE, so the
# branch has to come from a collaborator rather than from anyone at all, which
# narrows who can attempt it without closing the path: a compromised
# contributor account, or a workflow edit on a PR branch, still reaches it.
# Cache poisoning also survives the credential it was planted with, which is
# what makes it worth a second role rather than a trust condition.
# So writes are restricted to pushes on `main`, exactly as the
# `er-ci-ecr-push` role already restricts image pushes, and exactly as
# rust.yml's rust-cache configuration already restricts its single writer
# ("`lint` is the SINGLE WRITER of `shared-key: rust`, and only from `main`").
# PRs read the cache that `main` populated, which is where nearly all the value
# is: the dependency tree, not the PR's own crates.
#
# Fork PRs get neither role. GitHub does not issue an `id-token` to a workflow
# triggered by a fork's pull_request, so those runs simply have no cache — the
# safe failure, and not a regression, since the Actions backend was equally
# unavailable to them.
#
# The read-only half rests on sccache treating a denied cache write as a
# non-fatal miss, so that was MEASURED rather than assumed: sccache 0.19.0
# against a chmod-ed read-only cache directory, compiling a source change the
# cache could not satisfy, exits 0 and reports
#
#   Cache write errors                    1
#   Cache errors                          0
#   Compilation failures                  0
#
# — the write failure is counted, not raised. One caveat kept deliberately: the
# probe used the local-disk backend, not S3. sccache handles the failure at the
# same backend-agnostic `storage.put()` boundary, so this is strong evidence
# rather than proof that an S3 403 behaves identically. Watch
# `Cache write errors` on the first PR run after the switch; a 0 there together
# with a collapsed hit rate would mean the denial lands somewhere else.
#
# If that ever stops holding, the choice becomes one read+write role (accepting
# the poisoning surface above) or no cache on PRs at all.

locals {
  sccache_repo = "repo:athvin@273880343/entity-resolution-engine@1334344510"
}

data "aws_iam_policy_document" "sccache_trust_write" {
  statement {
    effect  = "Allow"
    actions = ["sts:AssumeRoleWithWebIdentity"]

    principals {
      type        = "Federated"
      identifiers = [module.github_oidc.oidc_provider_arn]
    }

    condition {
      test     = "StringEquals"
      variable = "token.actions.githubusercontent.com:aud"
      values   = ["sts.amazonaws.com"]
    }

    # Pushes on main only. Note this is the same immutable-subject form the
    # github-oidc module pins: owner and repo carry their numeric IDs, so the
    # claim survives a rename and cannot be reclaimed by re-registering a
    # deleted name.
    condition {
      test     = "StringEquals"
      variable = "token.actions.githubusercontent.com:sub"
      values   = ["${local.sccache_repo}:ref:refs/heads/main"]
    }
  }
}

data "aws_iam_policy_document" "sccache_trust_read" {
  statement {
    effect  = "Allow"
    actions = ["sts:AssumeRoleWithWebIdentity"]

    principals {
      type        = "Federated"
      identifiers = [module.github_oidc.oidc_provider_arn]
    }

    condition {
      test     = "StringEquals"
      variable = "token.actions.githubusercontent.com:aud"
      values   = ["sts.amazonaws.com"]
    }

    # Any ref of THIS repository — which is what lets a PR read. Still scoped
    # by the immutable owner/repo IDs, so no other repository can assume it.
    condition {
      test     = "StringLike"
      variable = "token.actions.githubusercontent.com:sub"
      values   = ["${local.sccache_repo}:*"]
    }
  }
}

resource "aws_iam_role" "sccache_write" {
  name                 = "er-ci-sccache-write"
  assume_role_policy   = data.aws_iam_policy_document.sccache_trust_write.json
  max_session_duration = 3600
}

resource "aws_iam_role" "sccache_read" {
  name                 = "er-ci-sccache-read"
  assume_role_policy   = data.aws_iam_policy_document.sccache_trust_read.json
  max_session_duration = 3600
}

data "aws_iam_policy_document" "sccache_write" {
  statement {
    effect    = "Allow"
    actions   = ["s3:GetObject", "s3:PutObject"]
    resources = ["${aws_s3_bucket.sccache.arn}/*"]
  }

  # sccache lists to probe the bucket; scoped to this bucket only.
  statement {
    effect    = "Allow"
    actions   = ["s3:ListBucket"]
    resources = [aws_s3_bucket.sccache.arn]
  }
}

data "aws_iam_policy_document" "sccache_read" {
  statement {
    effect    = "Allow"
    actions   = ["s3:GetObject"]
    resources = ["${aws_s3_bucket.sccache.arn}/*"]
  }

  statement {
    effect    = "Allow"
    actions   = ["s3:ListBucket"]
    resources = [aws_s3_bucket.sccache.arn]
  }

  # No DeleteObject in either policy: expiry is the lifecycle rule's job, and a
  # CI role that can delete cache entries can also empty the cache.
}

resource "aws_iam_role_policy" "sccache_write" {
  name   = "sccache-rw"
  role   = aws_iam_role.sccache_write.id
  policy = data.aws_iam_policy_document.sccache_write.json
}

resource "aws_iam_role_policy" "sccache_read" {
  name   = "sccache-ro"
  role   = aws_iam_role.sccache_read.id
  policy = data.aws_iam_policy_document.sccache_read.json
}

output "sccache_bucket" {
  value = aws_s3_bucket.sccache.id
}

output "sccache_write_role_arn" {
  value = aws_iam_role.sccache_write.arn
}

output "sccache_read_role_arn" {
  value = aws_iam_role.sccache_read.arn
}
