# The two roles CI assumes to run Terraform (infrastructure.md §14.2): plan on
# every PR under a read-only role, apply on `main` under the apply role, both
# through §11's GitHub OIDC provider with the same `main`-only `sub`
# restriction the ECR push role carries.
#
# NO REPOSITORY SECRETS. The account has never held a static CI key and this
# does not introduce one: `gh secret list` is empty, §11 records that CI
# "currently consumes zero repository secrets", and the SES work deliberately
# avoided minting a second static key (§9.3). A long-lived AWS access key in
# repository secrets is strictly worse than a federated role whose credentials
# last an hour and whose trust names one branch: it does not expire, every
# workflow in the repository can reach it rather than one job, and rotating it
# is a manual act nobody is prompted to perform.
#
# (The repository is PRIVATE — `gh repo view --json visibility`. Earlier
# comments here argued this from the repository being public, which it is not;
# the argument never depended on visibility, because repository secrets are not
# readable by anonymous visitors on a public repo either.)
#
# --- the bootstrap, stated plainly -------------------------------------------
#
# These roles cannot create themselves. The FIRST apply has to come from a human
# identity (`AWS_PROFILE=er-mgmt terraform -chdir=infra/terraform/envs/core
# apply`); after that CI owns every subsequent apply, including this file's own
# future changes. This removes the recurring manual apply, not the first one.
#
# --- §14.2's three roles, and which does what -------------------------------
#
# `er-ci-tf-plan`        read-only, any ref, every PR
# `er-ci-tf-apply`       `main` only, applies `core` and `dev`
# `er-ci-tf-apply-prod`  reachable from exactly two principals, applies `prod`
#
# The third is §14.2's prod-scoped apply role, built at the bottom of this file.
# It exists because applies are UNATTENDED: the required-reviewer environment
# that once covered a prod write stopped requiring anyone in #355, and prod has
# held an Aurora cluster since #352. Read the comment above the role for what
# the extra hop does and does not buy — it is narrower than it looks, and
# overstating it would be worse than not having it.

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
  #
  # Both state prefixes, because the plan tier covers more than `envs/`:
  # `bootstrap/state-backend` is PLANNED in CI and never applied there (it owns
  # the bucket its own backend lives in, so applying it from CI is the circular
  # case — but a plan still catches drift in the bucket's configuration).
  # `bootstrap/organization` is deliberately not planned at all: its one
  # required variable is a root account email kept out of version control, and
  # supplying it would mean introducing the first repository secret into a
  # pipeline whose stated property is that it has none.
  #
  # Enumerated rather than `/*`: the point of this statement is that the role
  # reads Terraform state and nothing else in the bucket.
  statement {
    effect  = "Allow"
    actions = ["s3:GetObject"]
    resources = [
      "${local.tf_state_arn}/envs/*.tfstate",
      "${local.tf_state_arn}/bootstrap/*.tfstate",
    ]
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
    # so "its trust policy is part of this claim".
    #
    # ENVIRONMENT subjects, not a ref subject. A job that references a GitHub
    # Environment gets a DIFFERENT `sub` claim: `…:environment:<name>` replaces
    # `…:ref:refs/heads/main`. The apply jobs each set
    # `environment: infra-apply-<env>`, so a ref-pinned condition can never
    # match them — the first real apply failed with exactly that, while `plan`
    # (which sets no environment) succeeded against the same provider from the
    # same branch. That contrast is the diagnosis.
    #
    # This is not weaker than the ref form. Each `infra-apply-*` environment
    # carries `deployment_branch_policy.protected_branches = true`, so only a
    # protected branch — `main` — can deploy to it, and GitHub enforces that
    # before the token is minted. The main-only property moves from the claim
    # to the environment rather than disappearing. Listing the three names
    # explicitly rather than globbing `:environment:*` keeps a new environment
    # from silently inheriting apply rights.
    condition {
      test     = "StringEquals"
      variable = "token.actions.githubusercontent.com:sub"
      values = [
        "${local.ci_tf_repo}:environment:infra-apply-core",
        "${local.ci_tf_repo}:environment:infra-apply-dev",
        "${local.ci_tf_repo}:environment:infra-apply-prod",
      ]
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

# THE ONE THING THIS ROLE MAY NOT DO: apply `envs/prod`.
#
# An explicit Deny overrides the AdministratorAccess Allow above, and the scope
# is the prod STATE OBJECT rather than a list of prod resource types. That choice
# is the whole trick here.
#
# Enumerating prod resources would be the instinct and would not work: the prod
# stack manages a VPC and its subnets, NAT, EKS, Aurora, EFS, S3 and KMS, and
# §8.2's PlatformAdmin guard — which does enumerate — deliberately covers only
# the destructive verbs on five services. Copying it here would leave the apply
# role able to reconfigure prod's network while being unable to delete its
# database, which is not a boundary anyone can reason about.
#
# Terraform cannot apply a stack it cannot read the state of, so denying these
# two objects denies the whole stack with one statement and no list to keep in
# step with the prod environment. `.tflock` is the second object because
# `use_lockfile = true` makes acquiring the lock an S3 write of its own.
#
# WHAT THIS IS AND IS NOT. It forces the prod write through
# `er-ci-tf-apply-prod`, so the audited hop is the only path Terraform has and
# cannot quietly stop being used — without it the chaining step below would be
# decorative, and removing that step would silently restore the old direct path.
# It is NOT containment: this role holds AdministratorAccess and may assume the
# prod role, so it reaches prod deliberately. What it stops is the accident —
# a matrix entry, a `-chdir`, or a copied job pointed at `envs/prod` from the
# non-prod identity now fails on state access instead of reconfiguring
# production.
data "aws_iam_policy_document" "ci_tf_apply_no_prod_state" {
  statement {
    sid     = "DenyProdStateAccess"
    effect  = "Deny"
    actions = ["s3:*"]
    resources = [
      "${local.tf_state_arn}/envs/prod.tfstate",
      "${local.tf_state_arn}/envs/prod.tfstate.tflock",
    ]
  }
}

resource "aws_iam_role_policy" "ci_tf_apply_no_prod_state" {
  name   = "deny-prod-state"
  role   = aws_iam_role.ci_tf_apply.id
  policy = data.aws_iam_policy_document.ci_tf_apply_no_prod_state.json
}

# --- prod: the second, separately auditable hop ------------------------------
#
# §14.2's prod-scoped apply role, which this file has carried a "NOT built yet"
# note about since the plan/apply gate landed. Applies are unattended, so the
# note's premise — "briefly covered by a required-reviewer environment" — has
# been false since #355, and prod has held an Aurora cluster since #352.
#
# WHAT THE HOP BUYS, stated honestly, because it is narrower than it looks.
#
#   1. Auditability. Every prod write is a distinct `AssumeRole` into a
#      prod-named role in CloudTrail, rather than being indistinguishable from
#      a dev write by the same principal, and the chained session carries
#      configure-aws-credentials' GitHub session tags — Repository, Workflow,
#      Actor, Commit, Branch — so the trail records who merged what. §8.6 can
#      alert on it.
#   2. No ACCIDENTAL prod apply from the non-prod path. The companion PR denies
#      `er-ci-tf-apply` the prod state object, so a workflow bug that pointed
#      the ordinary apply role at `envs/prod` fails on state access rather than
#      reconfiguring production.
#   3. A break-glass path with the SAME permissions CI uses, so a human
#      recovering prod by hand is not improvising a different identity.
#
# WHAT IT DOES NOT BUY: containment of a compromised `er-ci-tf-apply`. That role
# can assume this one — that is the entire design — so anyone holding it reaches
# prod through the front door. The control on that remains its own trust policy
# (`main`-only, via the three `infra-apply-*` environments) plus branch
# protection. Do not read this role as a boundary against the apply role; read it
# as making the prod write legible, and as making the non-prod path unable to
# touch prod by mistake.
#
# AdministratorAccess again, and for a sharper reason than above: narrowing it
# would buy nothing. The principals that can reach it are `er-ci-tf-apply`
# (AdministratorAccess) and `ProdAdmin` (AdministratorAccess, by
# `aws_ssoadmin_managed_policy_attachment.prod_admin`). A tighter policy here
# would only block Terraform, never an actor, while adding the mid-apply
# AccessDenied failure mode the comment above rejects.
data "aws_iam_policy_document" "ci_tf_apply_prod_trust" {
  # PRINCIPAL 1 of 2 — the CI apply role, named exactly. This is the hop the
  # `apply (prod)` job chains through.
  #
  # `sts:TagSession` alongside `sts:AssumeRole`, and it is load-bearing rather
  # than boilerplate. The other roles in this file are assumed through OIDC, and
  # configure-aws-credentials DELETES the session tags on both web-identity
  # paths (`delete params.Tags` in its assumeRole source) — which is why none of
  # their trust policies needs this. Role CHAINING takes the plain `AssumeRole`
  # path, which keeps them, so without this the prod apply fails on an
  # AccessDenied for TagSession and nothing else explains why.
  #
  # Allowing it is also the better end of the trade. The action tags the session
  # with Repository, Workflow, Actor, Commit and Branch, so the CloudTrail entry
  # for a prod write records WHO merged WHAT — which is reason 1 for this role
  # existing. Skipping the tags with `role-skip-session-tagging` would have
  # worked and would have thrown that away.
  statement {
    sid     = "CiApplyRoleChain"
    effect  = "Allow"
    actions = ["sts:AssumeRole", "sts:TagSession"]

    principals {
      type        = "AWS"
      identifiers = [aws_iam_role.ci_tf_apply.arn]
    }
  }

  # PRINCIPAL 2 of 2 — `ProdAdmin`, as break-glass.
  #
  # Expressed as the account root plus an `ArnLike` on the CALLING principal,
  # not as the SSO role's ARN directly, and the difference matters twice over.
  #
  # An Identity Center permission set's role is named
  # `AWSReservedSSO_ProdAdmin_<hash>`; `envs/prod/eks.tf` hardcodes today's
  # hash, which is fine for an EKS access entry and wrong for a trust policy:
  # IAM resolves a principal ARN to the role's unique id on write, so once the
  # permission set is recreated — any change to it does that — the trust policy
  # holds a dangling id that cannot be read back as a name. The pattern form
  # survives that.
  #
  # This is NOT "anyone in the account". A root principal delegates to identity
  # policies, and the condition is evaluated against the caller's own ARN, so
  # the effective set is exactly: principals whose ARN matches a ProdAdmin SSO
  # session AND that hold `sts:AssumeRole`. `PlatformAdmin` does not match the
  # pattern; neither does the `er-dev-builder` user's key.
  statement {
    sid     = "ProdAdminBreakGlass"
    effect  = "Allow"
    actions = ["sts:AssumeRole"]

    principals {
      type        = "AWS"
      identifiers = ["arn:aws:iam::${local.mgmt_account_id}:root"]
    }

    condition {
      test     = "ArnLike"
      variable = "aws:PrincipalArn"
      values = [
        "arn:aws:iam::${local.mgmt_account_id}:role/aws-reserved/sso.amazonaws.com/*/AWSReservedSSO_ProdAdmin_*",
      ]
    }
  }
}

resource "aws_iam_role" "ci_tf_apply_prod" {
  name = "er-ci-tf-apply-prod"
  # An hour, matching the apply role. A chained session is capped at one hour by
  # STS regardless of what is set here, so this is the honest value rather than
  # a longer one that would silently not apply.
  max_session_duration = 3600
  assume_role_policy   = data.aws_iam_policy_document.ci_tf_apply_prod_trust.json
}

resource "aws_iam_role_policy_attachment" "ci_tf_apply_prod_admin" {
  role       = aws_iam_role.ci_tf_apply_prod.name
  policy_arn = "arn:aws:iam::aws:policy/AdministratorAccess"
}

output "ci_tf_plan_role_arn" {
  value = aws_iam_role.ci_tf_plan.arn
}

output "ci_tf_apply_prod_role_arn" {
  value = aws_iam_role.ci_tf_apply_prod.arn
}

output "ci_tf_apply_role_arn" {
  value = aws_iam_role.ci_tf_apply.arn
}
