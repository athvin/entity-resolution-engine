# The role CI assumes to roll the dev platform release (infrastructure.md §14.2
# — "Dev platform releases apply from CI on `main`; `make dev-up` stays a human
# command").
#
# --- what this deploys INTO, and why it is a new namespace --------------------
#
# §14.2's clause had no target when it was written: `er-platform` is only ever
# installed into a PER-DEVELOPER namespace by `make dev-up` (its `developer`
# value defaults to the release namespace and names the §9 Secrets Manager
# entries `er/{environment}/{developer}/…`). There is no shared dev release, so
# the only readings of "applies from CI on main" were (a) redeploy every
# developer's namespace, which restarts a pod someone is mid-debug on and
# contradicts the very next clause, or (b) give CI a namespace of its own.
#
# (b), by operator decision. CI owns exactly one namespace and cannot touch a
# developer's. `make dev-up` is untouched.
#
# The namespace is **`ci`**, not `dev`. `dev` was the suggested example, but the
# name becomes the `developer` value, which lands in secret paths — `er/dev/dev/
# erserver` reads like a mistake, and `ci` additionally cannot collide with a
# real developer's DNS-safe name.
#
# --- not yet verified --------------------------------------------------------
#
# This role does not exist until `envs/dev` is applied, and nothing here has
# been exercised against the live cluster. The workflow carries a
# `workflow_dispatch` precisely so the first rollout can be a deliberate manual
# run rather than whatever the next merge happens to be.

locals {
  # The namespace, release name and `developer` value CI owns. Also the
  # Secrets Manager path segment, which is why it is a bare DNS-safe word.
  ci_namespace = "ci"
}

# The provider itself lives in envs/core (the github-oidc module owns the single
# per-issuer provider an account may hold). Read it rather than hardcode the
# ARN: this fails loudly if core has not been applied, which is the correct
# ordering signal.
data "aws_iam_openid_connect_provider" "github" {
  url = "https://token.actions.githubusercontent.com"
}

data "aws_iam_policy_document" "ci_deploy_dev_trust" {
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
    # reason: a PR from a fork runs attacker-authored workflow code.
    condition {
      test     = "StringEquals"
      variable = "token.actions.githubusercontent.com:sub"
      values   = ["repo:athvin@273880343/entity-resolution-engine@1334344510:ref:refs/heads/main"]
    }
  }
}

resource "aws_iam_role" "ci_deploy_dev" {
  name                 = "er-ci-deploy-dev"
  assume_role_policy   = data.aws_iam_policy_document.ci_deploy_dev_trust.json
  max_session_duration = 3600
}

data "aws_iam_policy_document" "ci_deploy_dev" {
  # Enough to build a kubeconfig. Cluster-level AUTHORIZATION is separate and
  # comes from the access entry in eks.tf — this only lets the role discover
  # the endpoint and CA.
  statement {
    effect    = "Allow"
    actions   = ["eks:DescribeCluster"]
    resources = [module.eks.cluster_arn]
  }

  # `scripts/dev_env.sh up` finds or creates the namespace's EFS access point.
  # Create is needed because the `ci` namespace has none until the first run;
  # Delete is deliberately absent, so CI can provision but never tear down.
  statement {
    effect = "Allow"
    actions = [
      "elasticfilesystem:DescribeFileSystems",
      "elasticfilesystem:DescribeAccessPoints",
      "elasticfilesystem:CreateAccessPoint",
      "elasticfilesystem:TagResource",
    ]
    resources = ["*"]
  }

  # The three §9 secrets for this namespace, and ONLY this namespace: the
  # resource pattern is pinned to `er/dev/ci/*`, so a bug in the workflow
  # cannot read a developer's credentials. No Delete, for the reason above.
  statement {
    effect = "Allow"
    actions = [
      "secretsmanager:CreateSecret",
      "secretsmanager:DescribeSecret",
      "secretsmanager:GetSecretValue",
      "secretsmanager:TagResource",
    ]
    resources = ["arn:aws:secretsmanager:us-east-2:${data.aws_caller_identity.current.account_id}:secret:er/dev/${local.ci_namespace}/*"]
  }

  # ONE shared secret, named exactly, and NOT `er/dev/shared/*`.
  #
  # `dev_env.sh up` reads `er/dev/shared/postgres` (its line 116) to derive the
  # namespace's database credentials; everything else it touches lives under
  # `er/dev/<namespace>/`, covered above. The first real deploy run failed here,
  # reporting the secret "missing" — it exists, but DescribeSecret was denied by
  # the narrower pattern, and the script cannot tell those apart.
  #
  # The tempting fix is `er/dev/shared/*`. That would also grant read of
  # `er/dev/shared/builder`, which holds the TEMPORARY never-expiring
  # AdministratorAccess key (builder.tf) — turning a namespace deploy role into
  # a path to account admin for anything that can run on `main`. Naming the one
  # secret keeps that closed.
  statement {
    effect    = "Allow"
    actions   = ["secretsmanager:GetSecretValue", "secretsmanager:DescribeSecret"]
    resources = ["arn:aws:secretsmanager:us-east-2:${data.aws_caller_identity.current.account_id}:secret:er/dev/shared/postgres-*"]
  }

  # The span store's root credentials and metadata DSN, read by
  # scripts/obs_up.sh when the deploy installs the observability stack.
  #
  # Named exactly, for the same reason the statement above is: `er/shared/*`
  # would be a standing grant over every future shared secret, and the whole
  # point of the comment above is that a namespace deploy role must not become
  # a path to anything it was not given deliberately.
  statement {
    effect    = "Allow"
    actions   = ["secretsmanager:GetSecretValue", "secretsmanager:DescribeSecret"]
    resources = ["arn:aws:secretsmanager:us-east-2:${data.aws_caller_identity.current.account_id}:secret:er/shared/openobserve-*"]
  }

  # Resolve an image digest from a tag. §14.2 requires the deploy to pass a
  # DIGEST and to gate on that digest being present, which is a read.
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
}

resource "aws_iam_role_policy" "ci_deploy_dev" {
  name   = "deploy-dev"
  role   = aws_iam_role.ci_deploy_dev.id
  policy = data.aws_iam_policy_document.ci_deploy_dev.json
}

output "ci_deploy_dev_role_arn" {
  value = aws_iam_role.ci_deploy_dev.arn
}

output "ci_namespace" {
  value = local.ci_namespace
}
