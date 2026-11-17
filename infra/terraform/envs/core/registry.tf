# Task 0.7 (infrastructure.md §11, rev 4): the account's single registry —
# immutable tags, scan-on-push, a manifest-count lifecycle. AES256 is the §8.3
# deliberate exception. Promotion is by digest, never by tag; replication
# dissolved with the account split.

locals {
  images = ["er-pipeline", "er-api", "er-web"]
}

resource "aws_ecr_repository" "this" {
  for_each = toset(local.images)

  name                 = each.key
  image_tag_mutability = "IMMUTABLE"

  image_scanning_configuration {
    scan_on_push = true
  }

  encryption_configuration {
    encryption_type = "AES256"
  }
}

# The budget is counted in MANIFESTS, and one build pushes three of them. That
# is the whole of #1101: "retain the last 30 images" sounds like weeks and was
# measured at ~1.5 days, which broke a dev namespace pinned to a 33-day-old tag
# with `ErrImagePull ... not found` — a message that reads as a registry fault.
#
# Measured on er-api, 2026-11-17 (48 manifests, 16 of them tagged):
#
#   16  tagged     application/vnd.oci.image.index.v1+json      <- what a tag names
#   16  UNTAGGED   application/vnd.oci.image.manifest.v1+json   <- the platform image
#   16  UNTAGGED   .../vnd.docker.attestation.manifest.v1+json  <- buildkit SLSA
#
# So 30 manifests is **ten pullable builds**, and at the measured ~10 builds/day
# that is the 1.5-day window.
#
# DO NOT "reclaim" the untagged two-thirds with an expire-untagged rule. The
# count symmetry above is not a coincidence: those untagged manifests are the
# live children of the tagged indexes, so expiring them deletes the platform
# image every tag resolves to. It looks like free headroom and is a way to break
# every image in the registry.
#
# Raising the count is therefore the only safe lever here, and it stays a count
# rather than becoming `sinceImagePushed`: ECR permits only one `tagStatus: any`
# rule, so an age rule cannot simply be added beside this one, and any
# tag-scoped rule reintroduces the hazard above.
#
# 900 manifests = 300 builds ~= 30 days at the measured rate, and longer while
# the repo is quiet, which is exactly the idle-dev-namespace case. Cost ceiling,
# using per-image sizes that double-count shared layers so the true figure is
# lower: 300 builds x (107 + 68 + 204) MB ~= 114 GB ~= $11/month at
# $0.10/GB-month. The three repos held 16.8 GB combined before this change.
resource "aws_ecr_lifecycle_policy" "this" {
  for_each   = aws_ecr_repository.this
  repository = each.value.name

  policy = jsonencode({
    rules = [{
      rulePriority = 1
      description  = "Retain the last 900 manifests (~300 builds; 3 manifests per build)"
      selection = {
        tagStatus   = "any"
        countType   = "imageCountMoreThan"
        countNumber = 900
      }
      action = { type = "expire" }
    }]
  })
}

# The buildkit layer cache (images.yml `cache-to: type=registry`). Its own
# repository, and the three departures from the image repos above are each the
# point, not drift:
#
#   - MUTABLE: a registry cache ref (`er-buildcache:er-api`) is overwritten on
#     every push; the image repos' IMMUTABLE would reject the second one.
#   - scan_on_push off: the cache holds layers of images that are themselves
#     scanned on push to their own repos — scanning the cache manifests would
#     double the findings for zero added coverage.
#   - its own lifecycle: each cache push replaces the tag and orphans the
#     previous manifest, so the real budget rule is expire-untagged; the image
#     repos' last-30-of-any would instead let orphans pool for a month.
#
# WHY this exists at all: the Actions cache is a shared 10 GB budget, and
# buildkit's `type=gha,mode=max` layer cache (~5 GB measured) was the largest
# producer in it — the thing rust.yml's `cache-prune` job exists to cap. The
# registry backend has no shared budget, the same move sccache.tf made.
# SEQUENCE (the sccache lesson): this must be APPLIED before images.yml points
# at it, or both image builds fail pushing cache to a repo that does not exist.
resource "aws_ecr_repository" "buildcache" {
  name                 = "er-buildcache"
  image_tag_mutability = "MUTABLE"

  image_scanning_configuration {
    scan_on_push = false
  }

  encryption_configuration {
    encryption_type = "AES256"
  }
}

resource "aws_ecr_lifecycle_policy" "buildcache" {
  repository = aws_ecr_repository.buildcache.name

  policy = jsonencode({
    rules = [
      {
        rulePriority = 1
        description  = "Expire orphaned cache manifests after a day"
        selection = {
          tagStatus   = "untagged"
          countType   = "sinceImagePushed"
          countUnit   = "days"
          countNumber = 1
        }
        action = { type = "expire" }
      },
      {
        rulePriority = 2
        description  = "Backstop: never hold more than 10 cache images"
        selection = {
          tagStatus   = "any"
          countType   = "imageCountMoreThan"
          countNumber = 10
        }
        action = { type = "expire" }
      }
    ]
  })
}

module "github_oidc" {
  source = "../../modules/github-oidc"

  repository    = "athvin/entity-resolution-engine"
  owner_id      = "273880343"
  repository_id = "1334344510"
  # The push role covers the cache repo too: images.yml's build jobs assume
  # one role for both the image push and the cache push.
  ecr_repository_arns = concat(
    [for r in aws_ecr_repository.this : r.arn],
    [aws_ecr_repository.buildcache.arn],
  )
}

output "ecr_push_role_arn" {
  value = module.github_oidc.role_arn
}

output "ecr_repository_urls" {
  value = { for k, r in aws_ecr_repository.this : k => r.repository_url }
}
