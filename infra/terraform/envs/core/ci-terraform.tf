# The two roles CI assumes to run Terraform (infrastructure.md §14.2): plan on
# every PR under a read-only role, apply on `main` under the apply role, both
# through §11's GitHub OIDC provider with the same `main`-only `sub`
# restriction the ECR push role carries.
#
# NO REPOSITORY SECRETS. The account has never held a static CI key and this
# does not introduce one: `gh secret list` is empty, §11 records that CI
# "currently consumes zero repository secrets", and the SES work deliberately
# avoided minting a second static key (§9.3). A long-lived AWS access key in a
# PUBLIC repository's secrets is strictly worse than a federated role whose
# credentials last an hour and whose trust names one branch.
#
# --- the bootstrap, stated plainly -------------------------------------------
#
# These roles cannot create themselves. The FIRST apply has to come from a human
# identity (`AWS_PROFILE=er-mgmt terraform -chdir=infra/terraform/envs/core
# apply`); after that CI owns every subsequent apply, including this file's own
# future changes. This removes the recurring manual apply, not the first one.
#
# --- what §14.2 asks for that this does NOT build yet ------------------------
#
# §14.2 also specifies a **prod-scoped** apply role whose trust policy names
# exactly two principals — the CI apply role and `ProdAdmin` as break-glass —
# so a prod write is a distinct, separately auditable hop. That second hop is
# NOT built here: `er-ci-tf-apply` applies all three envs directly. It was
# briefly covered by a required-reviewer environment on every apply; applies are
# now UNATTENDED by operator decision, so nothing covers it. This is the
# remaining defence-in-depth for prod and should land before prod carries data.

locals {
  # Same immutable-subject form the github-oidc module pins: owner and repo
  # carry their numeric IDs, so the claim survives a rename and cannot be
  # reclaimed by re-registering a deleted name.
  ci_tf_repo   = "repo:athvin@273880343/entity-resolution-engine@1334344510"
  tf_state_arn = "arn:aws:s3:::er-terraform-state-${local.mgmt_account_id}"
}

# --- plan: read-only, and reachable from a PR --------------------------------

data "aws_iam_policy_document" "ci_tf_plan_trust" {
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

    # ANY ref of this repository, unlike the apply role below — a plan has to
    # run on the pull request, which is the whole point of planning. Safe
    # because the attached policy is `ReadOnlyAccess` plus state READ: the worst
    # a malicious ref can do is read configuration it could also read by being
    # granted the `developers` group.
    #
    # Note a fork PR gets nothing regardless: GitHub issues no `id-token` to a
    # workflow triggered by a fork's `pull_request`, so the plan job degrades to
    # skipped rather than failing open.
    condition {
      test     = "StringLike"
      variable = "token.actions.githubusercontent.com:sub"
      values   = ["${local.ci_tf_repo}:*"]
    }
  }
}

resource "aws_iam_role" "ci_tf_plan" {
  name                 = "er-ci-tf-plan"
  assume_role_policy   = data.aws_iam_policy_document.ci_tf_plan_trust.json
  max_session_duration = 3600
}

resource "aws_iam_role_policy_attachment" "ci_tf_plan_readonly" {
  role       = aws_iam_role.ci_tf_plan.name
  policy_arn = "arn:aws:iam::aws:policy/ReadOnlyAccess"
}

data "aws_iam_policy_document" "ci_tf_plan_state" {
  # Read the state, and only the state. No Put and no Delete, which is why the
  # plan job must pass `-lock=false`: acquiring the S3 lockfile is a write.
  # A plan that cannot lock is correct-but-racy against a concurrent apply, and
  # the apply job's concurrency group is what actually serialises writes.
  statement {
    effect    = "Allow"
    actions   = ["s3:GetObject"]
    resources = ["${local.tf_state_arn}/envs/*.tfstate"]
  }

  statement {
    effect    = "Allow"
    actions   = ["s3:ListBucket"]
    resources = [local.tf_state_arn]
  }

  # ReadOnlyAccess does not grant KMS Decrypt, and the state bucket is
  # encrypted with a customer-managed key (§8.3), so without this every plan
  # fails on a GetObject it is otherwise allowed to make.
  statement {
    effect    = "Allow"
    actions   = ["kms:Decrypt", "kms:DescribeKey"]
    resources = ["*"]

    condition {
      test     = "StringEquals"
      variable = "kms:RequestAlias"
      values   = ["alias/er-terraform-state"]
    }
  }

}

resource "aws_iam_role_policy" "ci_tf_plan_state" {
  name   = "terraform-state-read"
  role   = aws_iam_role.ci_tf_plan.id
  policy = data.aws_iam_policy_document.ci_tf_plan_state.json
}

# --- apply: privileged, and reachable only from main -------------------------

data "aws_iam_policy_document" "ci_tf_apply_trust" {
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

    # THIS CONDITION IS THE SECURITY CONTROL. §8.2: the Terraform apply role
    # walks through the prod guardrail legitimately — the two deny statements
    # ride the *permission sets*, so they constrain humans, not this role — and
    # so "its trust policy is part of this claim". Pinned to pushes on `main`,
    # which is branch-protected and therefore reviewed, exactly as the ECR push
    # role is. Pull requests are excluded deliberately: a PR from a fork runs
    # attacker-authored workflow code (§11).
    condition {
      test     = "StringEquals"
      variable = "token.actions.githubusercontent.com:sub"
      values   = ["${local.ci_tf_repo}:ref:refs/heads/main"]
    }
  }
}

resource "aws_iam_role" "ci_tf_apply" {
  name = "er-ci-tf-apply"
  # One hour, not the 12 the API allows: an apply that has not finished in an
  # hour is stuck, and a shorter window bounds a leaked credential.
  max_session_duration = 3600
  assume_role_policy   = data.aws_iam_policy_document.ci_tf_apply_trust.json
}

# AdministratorAccess, deliberately, with the trust policy above as the control.
#
# A curated policy is the instinct and the wrong answer here: this Terraform
# manages IAM roles and policies, Identity Center permission sets and
# assignments, KMS keys, RDS, EKS and its addons, S3, ECR, Secrets Manager,
# Budgets and CUR exports. Enumerating that is a policy that silently fails on
# the next resource type with an opaque AccessDenied in the middle of an apply —
# a worse failure than the one it prevents, and it prevents little: a role that
# can write IAM can grant itself the rest.
#
# What makes this safe is not the policy, it is who can reach it: pushes on a
# branch-protected `main` only. Applies are unattended, so that trust condition
# plus PR review is the entire control — there is no approval step behind it.
resource "aws_iam_role_policy_attachment" "ci_tf_apply_admin" {
  role       = aws_iam_role.ci_tf_apply.name
  policy_arn = "arn:aws:iam::aws:policy/AdministratorAccess"
}

output "ci_tf_plan_role_arn" {
  value = aws_iam_role.ci_tf_plan.arn
}

output "ci_tf_apply_role_arn" {
  value = aws_iam_role.ci_tf_apply.arn
}
