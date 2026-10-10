# Task 0.7 (infrastructure.md §11): ECR in nonprod as the source of truth.
# Tag immutability + scan-on-push + last-30 lifecycle. AES256 is the §8.3
# deliberate exception — images hold no records. Replication to prod (plus
# the prod-side registry policy) lands once the prod account exists.

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

module "github_oidc" {
  source = "../../modules/github-oidc"

  repository          = "athvin/entity-resolution-engine"
  ecr_repository_arns = [for r in aws_ecr_repository.this : r.arn]
}

output "ecr_push_role_arn" {
  value = module.github_oidc.role_arn
}

output "ecr_repository_urls" {
  value = { for k, r in aws_ecr_repository.this : k => r.repository_url }
}
