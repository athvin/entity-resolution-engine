# The role CI assumes to roll the PROD platform release — the #556 deploy path,
# mirroring envs/dev/ci-deploy.tf with the prod differences called out per
# statement. infrastructure.md §14.2's prod hop: the Terraform half (the
# chained apply role) already exists; this file is the app-deploy half.
#
# --- the ordering this file exists to keep ------------------------------------
#
# This role must exist in the account BEFORE any workflow tries to assume it,
# which is why it lands in its own pull request (#929) ahead of the deploy.yml
# prod leg (#556). Dev set the precedent: its role was applied to the account
# first, and the workflow that uses it carries a `workflow_dispatch` so the
# first rollout was a deliberate manual run. A single PR carrying both would
# race on merge — the deploy fires within minutes on an images run whose build
# jobs skipped, while the infra apply that creates this role takes longer —
# and a red deploy on `main` is the noise deploy.yml's header exists to
# prevent.

locals {
  # The namespace, release name and `developer` value CI deploys into. Kept in
  # sync with secrets.tf's `local.app_namespace` BY VALUE rather than by
  # reference: the two files must agree, and a shared local would couple
  # unrelated refactors. The Secrets Manager path segment, which is why it is
  # a bare DNS-safe word.
  ci_prod_namespace = "app"
}

# The provider itself lives in envs/core (the github-oidc module owns the single
# per-issuer provider an account may hold). Read it rather than hardcode the
# ARN: this fails loudly if core has not been applied, which is the correct
# ordering signal. Same construction as dev.
data "aws_iam_openid_connect_provider" "github" {
  url = "https://token.actions.githubusercontent.com"
}

data "aws_iam_policy_document" "ci_deploy_prod_trust" {
  statement {
    effect  = "Allow"
    actions = ["sts:AssumeRoleWithWebIdentity"]

    principals {
      type        = "Federated"
      identifiers = [data.aws_iam_openid_connect_provider.github.arn]
    }

    condition {
      test     = "StringEquals"
      variable = "token.actions.githubusercontent.com:aud"
      values   = ["sts.amazonaws.com"]
    }

    # `main` only, the same restriction `er-ci-ecr-push` and `er-ci-tf-apply`
    # carry. A deploy is a mutation, so pull requests are excluded for the §11
    # reason: a PR from a fork runs attacker-authored workflow code. The
    # subject string is owner/repo BY ID so a rename cannot silently widen or
    # break the trust.
    condition {
      test     = "StringEquals"
      variable = "token.actions.githubusercontent.com:sub"
      values   = ["repo:athvin@273880343/entity-resolution-engine@1334344510:ref:refs/heads/main"]
    }
  }
}

resource "aws_iam_role" "ci_deploy_prod" {
  name                 = "er-ci-deploy-prod"
  assume_role_policy   = data.aws_iam_policy_document.ci_deploy_prod_trust.json
  max_session_duration = 3600
}

data "aws_iam_policy_document" "ci_deploy_prod" {
  # Enough to build a kubeconfig. Cluster-level AUTHORIZATION is separate and
  # comes from the access entry in eks.tf — this only lets the role discover
  # the endpoint and CA.
  statement {
    effect    = "Allow"
    actions   = ["eks:DescribeCluster"]
    resources = [module.eks.cluster_arn]
  }

  # READ-ONLY, where dev's role also Creates. Prod's storage is Terraform's:
  # efs.tf owns the `/app` access point (there is no dev_env.sh in prod — the
  # same reason secrets.tf owns the §9 entries), so the deploy DISCOVERS the
  # file system by its Name tag and the access point by its `/app` root path,
  # the same query dev_env.sh runs. A deploy that cannot find them fails
  # loudly rather than provisioning a shadow.
  statement {
    effect = "Allow"
    actions = [
      "elasticfilesystem:DescribeFileSystems",
      "elasticfilesystem:DescribeAccessPoints",
    ]
    resources = ["*"]
  }

  # READ-ONLY on the two §9 entries for the prod namespace, and ONLY that
  # namespace — same pin as dev, different reason: prod's entries are
  # Terraform-owned (secrets.tf), so Create here could only ever shadow or
  # fight the owner of record. The read exists for the deploy's pre-flight —
  # a release whose ExternalSecrets cannot resolve starts pods that hang
  # waiting on env, so the workflow verifies both entries exist and are
  # readable BEFORE helm runs, failing in seconds rather than after a rollout
  # timeout.
  statement {
    effect = "Allow"
    actions = [
      "secretsmanager:GetSecretValue",
      "secretsmanager:DescribeSecret",
    ]
    resources = [
      "arn:aws:secretsmanager:us-east-2:${data.aws_caller_identity.current.account_id}:secret:er/prod/${local.ci_prod_namespace}/*",
      aws_secretsmanager_secret.openobserve.arn,
    ]
  }

  # Resolve an image digest from a tag. §14.2 requires the deploy to pass a
  # DIGEST and to gate on that digest being present, which is a read. Same as
  # dev: one registry, one account.
  statement {
    effect = "Allow"
    actions = [
      "ecr:GetAuthorizationToken",
      "ecr:BatchGetImage",
      "ecr:DescribeImages",
      "ecr:ListImages",
    ]
    resources = ["*"]
  }

  # Resolve the edge target group BY NAME (er-prod-web) so the chart's
  # TargetGroupBinding gets its ARN. Discovered rather than committed: the ARN
  # changes if the group is ever recreated, and a hardcoded value would bind
  # to nothing while reading as configured — the same failure shape the
  # staleness-refuse in deploy-resolve-digests.sh exists to catch. Not
  # resource-scopable (a Describe), and metadata only.
  statement {
    effect    = "Allow"
    actions   = ["elasticloadbalancing:DescribeTargetGroups"]
    resources = ["*"]
  }
}

resource "aws_iam_role_policy" "ci_deploy_prod" {
  name   = "deploy-prod"
  role   = aws_iam_role.ci_deploy_prod.id
  policy = data.aws_iam_policy_document.ci_deploy_prod.json
}

output "ci_deploy_prod_role_arn" {
  value = aws_iam_role.ci_deploy_prod.arn
}
