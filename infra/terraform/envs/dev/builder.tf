# ─── TEMPORARY MVP BUILD CREDENTIAL — DEV-SCOPED, PROD-GUARDED ──────────────
#
# A long-lived, never-expiring access key for the autonomous MVP build loop.
# SSO PlatformAdmin sessions cap at PT8H (modules/identity-center/main.tf:67)
# and refresh only through an interactive browser login, so they cannot drive an
# unattended loop. A static IAM access key is the only credential that survives
# indefinitely headless.
#
# §8.1 EXCEPTION (long-lived human-minted key), §8.2 HONOURED (prod-guarded):
#   - It holds AdministratorAccess MINUS the §8.2 DenyProd* denies below, so it
#     is full-power over dev but CANNOT mutate er-prod-* — even though prod now
#     lives in the same account (rev 4). That guard is what lets this key coexist
#     with prod instead of being a prod-gate blocker. The denies mirror the
#     PlatformAdmin guard in modules/identity-center/main.tf:84-145, with ONE
#     deliberate omission: `iam:CreateAccessKey` is NOT denied, because the loop
#     must create the dev lake's access key (lake.tf).
#   - It is still a leak-sensitive credential; treat er/dev/shared/builder as
#     sensitive. It should still be removed when the MVP build loop is done
#     (`git rm` + apply) — it is just no longer a prerequisite for standing up
#     prod.
#
# APPLY IDENTITY: PlatformAdmin cannot create this (its own DenyAccessKeyCreation
# blocks iam:CreateAccessKey). Apply under the bootstrap admin:
#     AWS_PROFILE=er-mgmt terraform -chdir=infra/terraform/envs/dev apply
#
# REMOVAL (when the MVP build loop ends): `git rm` this file + apply; destroys
# the user, key, guard, and secret in one diff. Nothing else references it.
# ────────────────────────────────────────────────────────────────────────────

resource "aws_iam_user" "builder" {
  name = "er-dev-builder"

  tags = {
    Retire      = "when-build-loop-done"
    ExceptionTo = "infrastructure.md-8.1-no-longlived-human-keys"
    Purpose     = "temporary MVP autonomous-build-loop credential - dev-scoped prod-guarded"
  }
}

# Full dev power.
resource "aws_iam_user_policy_attachment" "builder_admin" {
  user       = aws_iam_user.builder.name
  policy_arn = "arn:aws:iam::aws:policy/AdministratorAccess"
}

# The §8.2 prod guard: an explicit Deny overrides the AdministratorAccess Allow,
# so this key cannot mutate any er-prod-* resource. Mirrors the PlatformAdmin
# inline guard (modules/identity-center/main.tf:84-145) EXCEPT it omits the
# DenyAccessKeyCreation statement — the dev build loop legitimately creates the
# lake IAM user's access key (lake.tf), and denying it would break `dev` applies.
resource "aws_iam_user_policy" "builder_prod_guard" {
  name = "er-dev-builder-prod-guard"
  user = aws_iam_user.builder.name

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Sid    = "DenyProdS3Mutation"
        Effect = "Deny"
        Action = ["s3:Put*", "s3:Delete*"]
        Resource = [
          "arn:aws:s3:::er-prod-*",
          "arn:aws:s3:::er-prod-*/*",
        ]
      },
      {
        Sid    = "DenyProdRdsMutation"
        Effect = "Deny"
        Action = ["rds:Delete*", "rds:Modify*"]
        Resource = [
          "arn:aws:rds:*:*:cluster:er-prod-*",
          "arn:aws:rds:*:*:db:er-prod-*",
        ]
      },
      {
        Sid    = "DenyProdEksMutation"
        Effect = "Deny"
        Action = ["eks:Delete*", "eks:Update*"]
        Resource = [
          "arn:aws:eks:*:*:cluster/er-prod-*",
          "arn:aws:eks:*:*:nodegroup/er-prod-*/*/*",
        ]
      },
      {
        Sid      = "DenyProdSecretsMutation"
        Effect   = "Deny"
        Action   = ["secretsmanager:Put*", "secretsmanager:Delete*"]
        Resource = "arn:aws:secretsmanager:*:*:secret:er/prod/*"
      },
      {
        Sid      = "DenyProdKeyDeletion"
        Effect   = "Deny"
        Action   = ["kms:ScheduleKeyDeletion"]
        Resource = "*"
        Condition = {
          StringEquals = { "aws:ResourceTag/Environment" = "prod" }
        }
      },
      {
        Sid    = "DenyProdRoleMinting"
        Effect = "Deny"
        Action = [
          "iam:CreateRole",
          "iam:PutRolePolicy",
          "iam:AttachRolePolicy",
          "iam:UpdateAssumeRolePolicy",
          "iam:DeleteRolePolicy",
          "iam:DetachRolePolicy",
        ]
        Resource = "arn:aws:iam::*:role/er-prod-*"
      },
    ]
  })
}

resource "aws_iam_access_key" "builder" {
  user = aws_iam_user.builder.name
}

# Key kept out of git — same pattern as the §9.2 lake credential (lake.tf). The
# loop reads it from here once into a local `er-builder` profile.
resource "aws_secretsmanager_secret" "builder" {
  name        = "er/dev/shared/builder"
  description = "TEMPORARY MVP build-loop key (dev-scoped, prod-guarded). Remove with builder.tf when the build loop is done."
}

resource "aws_secretsmanager_secret_version" "builder" {
  secret_id = aws_secretsmanager_secret.builder.id

  secret_string = jsonencode({
    AWS_ACCESS_KEY_ID     = aws_iam_access_key.builder.id
    AWS_SECRET_ACCESS_KEY = aws_iam_access_key.builder.secret
    AWS_REGION            = "us-east-2"
  })
}

# ARN only — never output the key material itself (it would land in plan logs).
output "builder_secret_arn" {
  value = aws_secretsmanager_secret.builder.arn
}
