# GitHub OIDC provider + the CI image-push role (infrastructure.md §11).
# The sub claim is pinned to pushes on main — PRs are excluded deliberately:
# a PR from a fork runs attacker-authored workflow code and must never hold
# a push credential.

variable "repository" {
  description = "owner/name of the GitHub repository allowed to push."
  type        = string
}

variable "branch" {
  type    = string
  default = "main"
}

variable "ecr_repository_arns" {
  description = "ECR repositories the role may push to."
  type        = list(string)
}

resource "aws_iam_openid_connect_provider" "github" {
  url            = "https://token.actions.githubusercontent.com"
  client_id_list = ["sts.amazonaws.com"]

  # AWS validates GitHub's OIDC cert against its own trust store; thumbprints
  # are required by the API but no longer load-bearing.
  thumbprint_list = [
    "6938fd4d98bab03faadb97b34396831e3780aea1",
    "1c58a3a8518e8759bf075b76b750d4f2df264fcd",
  ]
}

data "aws_iam_policy_document" "trust" {
  statement {
    effect  = "Allow"
    actions = ["sts:AssumeRoleWithWebIdentity"]

    principals {
      type        = "Federated"
      identifiers = [aws_iam_openid_connect_provider.github.arn]
    }

    condition {
      test     = "StringEquals"
      variable = "token.actions.githubusercontent.com:aud"
      values   = ["sts.amazonaws.com"]
    }

    condition {
      test     = "StringEquals"
      variable = "token.actions.githubusercontent.com:sub"
      values   = ["repo:${var.repository}:ref:refs/heads/${var.branch}"]
    }
  }
}

resource "aws_iam_role" "ecr_push" {
  name                 = "er-ci-ecr-push"
  assume_role_policy   = data.aws_iam_policy_document.trust.json
  max_session_duration = 3600
}

data "aws_iam_policy_document" "push" {
  statement {
    sid       = "EcrAuth"
    effect    = "Allow"
    actions   = ["ecr:GetAuthorizationToken"]
    resources = ["*"]
  }

  statement {
    sid    = "EcrPushPull"
    effect = "Allow"
    actions = [
      "ecr:BatchCheckLayerAvailability",
      "ecr:BatchGetImage",
      "ecr:GetDownloadUrlForLayer",
      "ecr:InitiateLayerUpload",
      "ecr:UploadLayerPart",
      "ecr:CompleteLayerUpload",
      "ecr:PutImage",
      "ecr:DescribeImages",
      "ecr:DescribeRepositories",
    ]
    resources = var.ecr_repository_arns
  }
}

resource "aws_iam_role_policy" "push" {
  name   = "ecr-push"
  role   = aws_iam_role.ecr_push.id
  policy = data.aws_iam_policy_document.push.json
}

output "role_arn" {
  value = aws_iam_role.ecr_push.arn
}
