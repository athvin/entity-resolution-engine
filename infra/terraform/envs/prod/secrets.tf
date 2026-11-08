# §9: the prod half of External Secrets Operator, and the two Secrets Manager
# entries the `er-platform` release reads.
#
# envs/dev has the ESO role and nothing else — the SM entries themselves are
# written by `scripts/dev_env.sh up`, which is correct there (a namespace per
# developer, created by a human command, so Terraform cannot know the list).
# Prod has exactly one namespace and no `dev_env.sh`, so the entries are
# Terraform's here. That is the whole difference between the two files.
#
# `local.app_namespace` is "app", not "prod". The chart turns the namespace into
# its `developer` value and that lands in the secret path, so "prod" would read
# back as `er/prod/prod/erserver` — the same objection ci-deploy.tf raised
# against naming dev's CI namespace `dev`.

locals {
  # The namespace, Helm release target and `developer` value for the prod
  # platform release; also the middle segment of the §9 secret paths.
  app_namespace = "app"

  secret_base = "er/prod/${local.app_namespace}"
}

# --- ESO's own identity ------------------------------------------------------

data "aws_iam_policy_document" "eso_trust" {
  statement {
    effect  = "Allow"
    actions = ["sts:AssumeRole", "sts:TagSession"]

    principals {
      type        = "Service"
      identifiers = ["pods.eks.amazonaws.com"]
    }
  }
}

resource "aws_iam_role" "eso" {
  name               = "er-prod-external-secrets"
  assume_role_policy = data.aws_iam_policy_document.eso_trust.json
}

# Scoped to `er/prod/*`, where the dev role allows `er/*`.
#
# Dev's breadth is harmless in the direction that matters — a dev-cluster ESO
# reading a prod secret is the leak, not the reverse — but it is the wrong
# default to copy into the environment that holds the real credentials. Prod's
# ESO has no business reading a developer's namespace, and narrowing it here
# costs nothing because prod's own entries are all under this prefix by
# construction (`local.secret_base`, and rds.tf's `er/prod/shared/postgres`).
data "aws_iam_policy_document" "eso" {
  statement {
    sid    = "ReadProdSecrets"
    effect = "Allow"
    actions = [
      "secretsmanager:GetSecretValue",
      "secretsmanager:DescribeSecret",
      "secretsmanager:ListSecretVersionIds",
    ]
    resources = ["arn:aws:secretsmanager:us-east-2:${data.aws_caller_identity.current.account_id}:secret:er/prod/*"]
  }

  # ESO's discovery calls are not resource-scopable: both actions are
  # account-wide in IAM and return metadata only. The grant that matters is the
  # one above — a listed secret whose value cannot be read is not a disclosure.
  statement {
    sid       = "ListForDiscovery"
    effect    = "Allow"
    actions   = ["secretsmanager:ListSecrets", "secretsmanager:BatchGetSecretValue"]
    resources = ["*"]
  }
}

resource "aws_iam_role_policy" "eso" {
  name   = "read-prod-secrets"
  role   = aws_iam_role.eso.id
  policy = data.aws_iam_policy_document.eso.json
}

resource "aws_eks_pod_identity_association" "eso" {
  cluster_name    = module.eks.cluster_name
  namespace       = "external-secrets"
  service_account = "external-secrets"
  role_arn        = aws_iam_role.eso.arn
}

# --- The two §9 entries the chart's ExternalSecrets extract ------------------
#
# Key lists are the chart's contract, not a choice: `externalsecrets.yaml` uses
# `dataFrom.extract`, so every JSON key becomes a Secret key and reaches the
# containers through `envFrom` verbatim. The authority on which keys exist is
# `scripts/dev_env.sh` (the dev writer) plus `_helpers.tpl`'s `secretKeyRef`
# lookups — anything renamed here fails as a missing env var at pod start, not
# at apply.

# The control plane's operator token: 32 random bytes as hex, which is exactly
# what dev's `secrets.token_hex(32)` produces.
resource "random_bytes" "operator_token" {
  length = 32
}

resource "aws_secretsmanager_secret" "erserver" {
  name        = "${local.secret_base}/erserver"
  description = "Prod control-plane env (er-server): DSN, operator token, tenant env template."
}

resource "aws_secretsmanager_secret_version" "erserver" {
  secret_id = aws_secretsmanager_secret.erserver.id

  secret_string = jsonencode({
    # The master role against the one prod database. Dev gives each developer
    # its own role and database and bootstraps the role from an in-cluster Job;
    # prod has one application, so the extra role would need that same Job to
    # exist here for no isolation gain — there is nothing to be isolated FROM.
    # Tenant separation in prod is DuckLake catalogs (§7.3), not Postgres roles.
    #
    # KNOWN GAP, tracked by #553: `sslmode=require` encrypts but does not verify
    # the server certificate, so it does not satisfy §3.1's `verify-full` probe.
    # Moving it needs the RDS CA bundle in the images first — an image change,
    # deliberately not smuggled into a Terraform PR. rds.tf's
    # `er/prod/shared/postgres` carries the same `require` for the same reason.
    ERSERVER_DSN = "postgresql://er:${random_password.db.result}@${aws_rds_cluster.prod.endpoint}:5432/er?sslmode=require"

    ERSERVER_OPERATOR_TOKEN = random_bytes.operator_token.hex

    # Per-tenant engine env. Prod sizing is above dev's 2 threads / 2GB, and
    # bounded by §5's `le=32` cap on duckdb_threads rather than by this value.
    ERSERVER_TENANT_ENV_JSON = jsonencode({
      ER_LAKE_ALIAS          = "lake"
      ER_DUCKDB_THREADS      = "4"
      ER_DUCKDB_MEMORY_LIMIT = "8GB"
    })
  })
}

resource "random_bytes" "web_session_secret" {
  length = 32
}

# `ERWEB_CREDENTIAL_KEY` is 32 raw bytes, base64-encoded —
# `frontend/src/lib/env.ts` enforces that shape and rejects anything else at
# boot. `random_bytes` exists for exactly this and gives `.base64` directly;
# base64-encoding a `random_password` would encode its 32 ASCII characters,
# which decodes to 32 bytes and so passes the check while carrying ~6 bits per
# byte of entropy instead of 8.
resource "random_bytes" "web_credential_key" {
  length = 32
}

resource "aws_secretsmanager_secret" "erweb" {
  name        = "${local.secret_base}/erweb"
  description = "Prod BFF env (er-web): database URL, session secret, credential-sealing key."

  # This key unseals `erweb.org_credentials` (§9.1). A rotation that loses the
  # old value makes every stored org credential unreadable, and the recovery
  # window is the only thing between a mistaken delete and that outcome.
  recovery_window_in_days = 30
}

resource "aws_secretsmanager_secret_version" "erweb" {
  secret_id = aws_secretsmanager_secret.erweb.id

  secret_string = jsonencode({
    ERWEB_DATABASE_URL   = "postgresql://er:${random_password.db.result}@${aws_rds_cluster.prod.endpoint}:5432/er?sslmode=require"
    ERWEB_SESSION_SECRET = random_bytes.web_session_secret.hex
    ERWEB_CREDENTIAL_KEY = random_bytes.web_credential_key.base64
    # Set out of band — a model key is not Terraform's to mint, and an empty
    # value is the honest default (the schema-AI features degrade rather than
    # crash). The ERWEB_OIDC_* pairs land here with Track S.
    ANTHROPIC_API_KEY = ""
  })

  # Never re-write this entry from Terraform once it exists. ANTHROPIC_API_KEY
  # is filled in by hand, and the sealing key must survive every apply — a
  # provider upgrade that re-rendered `secret_string` would rotate the key and
  # orphan every sealed org credential. Changing a value here is a deliberate
  # `aws secretsmanager put-secret-value`, not an apply.
  lifecycle {
    ignore_changes = [secret_string]
  }
}

output "app_namespace" {
  description = "The namespace, Helm release target and `developer` value for the prod platform release."
  value       = local.app_namespace
}

output "eso_role_arn" {
  value = aws_iam_role.eso.arn
}
