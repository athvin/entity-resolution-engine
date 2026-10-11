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
