# Task 0.7 (infrastructure.md §11, rev 4): the account's single registry —
# immutable tags, scan-on-push, last-30 lifecycle. AES256 is the §8.3
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

resource "aws_ecr_lifecycle_policy" "this" {
  for_each   = aws_ecr_repository.this
  repository = each.value.name

  policy = jsonencode({
    rules = [{
      rulePriority = 1
      description  = "Retain the last 30 images"
      selection = {
        tagStatus   = "any"
        countType   = "imageCountMoreThan"
        countNumber = 30
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
