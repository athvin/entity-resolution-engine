# Task 1.8 (§10.5/§10.6 Phase 1): the observability floor's AWS side.
#
# Two log groups, two Pod Identity roles. The split (§10.4): application/debug
# output expires at 30 days because it is a cost problem; audit trails need 400
# days, but none of them are CloudWatch log groups in this environment —
# CloudTrail's group lives in envs/core (Phase 0) and `erweb.audit` /
# erserver's `audit_log` are Postgres tables. So dev carries only the 30-day
# tier, named under /er/dev/*.
#
# Both roles follow the §9/1.6 convention (efs.tf, secrets.tf): EKS Pod
# Identity — trust pods.eks.amazonaws.com, bind to a ServiceAccount via
# aws_eks_pod_identity_association. No IRSA, no node-role credentials (the
# §5.6 IMDS hop limit makes the node path unreachable from pods anyway).

# --- log groups -------------------------------------------------------------

# Container stdout/stderr from every namespace, shipped by the Fluent Bit
# DaemonSet (filelog tail of /var/log/containers). This is the group the
# §10.4 PII-grep probe reads.
resource "aws_cloudwatch_log_group" "containers" {
  name              = "/er/dev/containers"
  retention_in_days = 30
}

# The awsemf exporter's output: EMF-structured events CloudWatch extracts
# metric data from (namespace ER/Dev). The extracted metrics live in CW
# Metrics on its own retention; these source events are debug-tier.
resource "aws_cloudwatch_log_group" "metrics" {
  name              = "/er/dev/metrics"
  retention_in_days = 30
}

# --- log shipper (Fluent Bit DaemonSet) -------------------------------------

data "aws_iam_policy_document" "obs_pod_trust" {
  statement {
    effect  = "Allow"
    actions = ["sts:AssumeRole", "sts:TagSession"]

    principals {
      type        = "Service"
      identifiers = ["pods.eks.amazonaws.com"]
    }
  }
}

resource "aws_iam_role" "fluent_bit" {
  name               = "er-dev-fluent-bit"
  assume_role_policy = data.aws_iam_policy_document.obs_pod_trust.json
}

data "aws_iam_policy_document" "fluent_bit" {
  # Streams only — deliberately no logs:CreateLogGroup, so the shipper can
  # never invent an unmanaged (retention-less) group; groups are Terraform's.
  statement {
    sid    = "ShipContainerLogs"
    effect = "Allow"
    actions = [
      "logs:CreateLogStream",
      "logs:DescribeLogStreams",
      "logs:PutLogEvents",
    ]
    resources = [
      aws_cloudwatch_log_group.containers.arn,
      "${aws_cloudwatch_log_group.containers.arn}:*",
    ]
  }
}

resource "aws_iam_role_policy" "fluent_bit" {
  name   = "ship-container-logs"
  role   = aws_iam_role.fluent_bit.id
  policy = data.aws_iam_policy_document.fluent_bit.json
}

resource "aws_eks_pod_identity_association" "fluent_bit" {
  cluster_name    = module.eks.cluster_name
  namespace       = "observability"
  service_account = "fluent-bit"
  role_arn        = aws_iam_role.fluent_bit.arn
}

# --- OTel collector gateway (Deployment) ------------------------------------

resource "aws_iam_role" "otel_collector" {
  name               = "er-dev-otel-collector"
  assume_role_policy = data.aws_iam_policy_document.obs_pod_trust.json
}

data "aws_iam_policy_document" "otel_collector" {
  # awsemf writes metric events into /er/dev/metrics. CreateLogGroup is
  # granted but scoped to that one group: the exporter recreates group+stream
  # on PutLogEvents failure, and "can recreate exactly the group Terraform
  # manages" is safe where a blanket CreateLogGroup is not.
  statement {
    sid    = "EmfMetricEvents"
    effect = "Allow"
    actions = [
      "logs:CreateLogGroup",
      "logs:CreateLogStream",
      "logs:DescribeLogStreams",
      "logs:PutLogEvents",
      "logs:PutRetentionPolicy",
    ]
    resources = [
      aws_cloudwatch_log_group.metrics.arn,
      "${aws_cloudwatch_log_group.metrics.arn}:*",
    ]
  }

  # X-Ray's write/sampling APIs take no resource-level scoping.
  statement {
    sid    = "XrayTraces"
    effect = "Allow"
    actions = [
      "xray:PutTraceSegments",
      "xray:PutTelemetryRecords",
      "xray:GetSamplingRules",
      "xray:GetSamplingTargets",
      "xray:GetSamplingStatisticSummaries",
    ]
    resources = ["*"]
  }

  # Not exercised by awsemf (which rides PutLogEvents), but the §10.5 promise
  # is a swappable exporter: the alternative CloudWatch metrics path is
  # PutMetricData, held here scoped to the one namespace so the swap is a
  # config change, not an IAM change.
  statement {
    sid       = "PutMetricDataErDev"
    effect    = "Allow"
    actions   = ["cloudwatch:PutMetricData"]
    resources = ["*"]

    condition {
      test     = "StringEquals"
      variable = "cloudwatch:namespace"
      values   = ["ER/Dev"]
    }
  }
}

resource "aws_iam_role_policy" "otel_collector" {
  name   = "export-metrics-traces"
  role   = aws_iam_role.otel_collector.id
  policy = data.aws_iam_policy_document.otel_collector.json
}

resource "aws_eks_pod_identity_association" "otel_collector" {
  cluster_name    = module.eks.cluster_name
  namespace       = "observability"
  service_account = "otel-collector"
  role_arn        = aws_iam_role.otel_collector.arn
}

# --- the shared span store's cluster binding --------------------------------
#
# The OpenObserve role and bucket live in `envs/core` because ONE store serves
# every environment (core/observability.tf). The Pod Identity association has
# to live here, though: it names a cluster, and the cluster is a dev resource.
#
# The ARN is CONSTRUCTED, not looked up, and that is a correction rather than a
# shortcut. A `data "aws_iam_role"` here failed `plan (dev)` outright with
# "couldn't find resource": a data source resolves at PLAN time, core and dev
# plan in the same CI run, so dev could never plan until core had already
# applied — a chicken-and-egg that red-lights the PR permanently rather than
# "enforcing an order of operations", which is what an earlier version of this
# comment claimed.
#
# A constructed ARN is plan-safe because it is a string. The cost is that a
# missing role surfaces at APPLY rather than at plan, which is the right place
# for it: apply is where the dependency actually matters, and `envs/core`
# applies before `envs/dev` in the same workflow. eks.tf constructs the one ARN
# it needs from core the same way.
resource "aws_eks_pod_identity_association" "openobserve" {
  cluster_name    = module.eks.cluster_name
  namespace       = "observability"
  service_account = "openobserve"
  role_arn        = "arn:aws:iam::${data.aws_caller_identity.current.account_id}:role/er-openobserve"
}

# --- the span store's credentials --------------------------------------------
#
# Terraform mints these so there is no manual step before the first deploy.
# They live in `envs/dev` rather than beside the bucket in `envs/core` for one
# reason: the metadata DSN names a Postgres, and the only Postgres in this
# account is the dev Aurora (rds.tf). The STORE is shared across environments;
# its metadata happens to live in the one database that exists.
#
# The database itself is NOT created here. OpenObserve creates its own tables
# on first boot, and `er` is the only database Aurora was provisioned with — a
# dedicated `openobserve` database would need a CREATE DATABASE this stack has
# no connection to run. Keeping it in `er` under its own tables is the honest
# trade: still unreachable from a tenant's credentials (a tenant gets
# `er_t_<org>`), and one less bootstrap step.

resource "random_password" "openobserve_root" {
  length = 32
  # The chart's own default is "Complexpass#123"; OpenObserve requires a
  # mixed-class password, so the character set is pinned rather than left to
  # chance on a regenerate.
  override_special = "!#$%*()-_=+[]{}<>:?"
  min_upper        = 2
  min_lower        = 2
  min_numeric      = 2
  min_special      = 2
}

resource "aws_secretsmanager_secret" "openobserve" {
  name        = "er/shared/openobserve"
  description = "Root credentials and metadata DSN for the shared span store; consumed by scripts/obs_up.sh (and the CI deploy that calls it)."
}

resource "aws_secretsmanager_secret_version" "openobserve" {
  secret_id = aws_secretsmanager_secret.openobserve.id

  secret_string = jsonencode({
    root_email    = "observability@dupezero.com"
    root_password = random_password.openobserve_root.result
    # URI form, which is what OpenObserve's ZO_META_POSTGRES_DSN expects —
    # not the libpq keyword form the control plane uses.
    meta_dsn = "postgres://er:${random_password.db.result}@${aws_rds_cluster.dev.endpoint}:5432/er"
  })
}

# The collector authenticates to the store with the same root credential, as
# HTTP Basic. Scoped down to an ingest-only user once the store is up and its
# own user management is reachable; until then this is one credential rather
# than a second unmanaged one.
output "openobserve_secret_arn" {
  description = "er/shared/openobserve — read by scripts/obs_up.sh at deploy time."
  value       = aws_secretsmanager_secret.openobserve.arn
}
