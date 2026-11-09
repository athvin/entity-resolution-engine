# Production log and metric shippers. The dedicated trace store and its
# identity/credentials live in span-store.tf (owner decision, #944).

# --- log groups -------------------------------------------------------------

# Container stdout/stderr from every namespace, shipped by the Fluent Bit
# DaemonSet. INFREQUENT_ACCESS at CREATION — dev had to destroy-and-recreate
# to reach this class (its file carries why that was the right trade there);
# prod simply starts here. Same give-ups as dev: no metric filters, no Live
# Tail, no subscription filters. Logs Insights still works on IA, and an
# ingest alarm reads AWS/Logs IncomingBytes — a CloudWatch-published metric,
# not a metric filter — so IA blinds neither.
resource "aws_cloudwatch_log_group" "containers" {
  name              = "/er/prod/containers"
  retention_in_days = 30
  log_group_class   = "INFREQUENT_ACCESS"
}

# The awsemf exporter's output: EMF-structured events CloudWatch extracts
# metric data from (namespace ER/Prod). The extracted metrics live in CW
# Metrics on their own retention; these source events are debug-tier.
resource "aws_cloudwatch_log_group" "metrics" {
  name              = "/er/prod/metrics"
  retention_in_days = 30
}

# Both roles reuse pod_identity.tf's `pod_trust` document rather than
# declaring a second copy of the same statement (dev's observability.tf
# predates that sharing).

# --- log shipper (Fluent Bit DaemonSet) -------------------------------------

resource "aws_iam_role" "fluent_bit" {
  name               = "er-prod-fluent-bit"
  assume_role_policy = data.aws_iam_policy_document.pod_trust.json
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
  name               = "er-prod-otel-collector"
  assume_role_policy = data.aws_iam_policy_document.pod_trust.json
}

data "aws_iam_policy_document" "otel_collector" {
  # awsemf writes metric events into /er/prod/metrics. CreateLogGroup is
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
    sid       = "PutMetricDataErProd"
    effect    = "Allow"
    actions   = ["cloudwatch:PutMetricData"]
    resources = ["*"]

    condition {
      test     = "StringEquals"
      variable = "cloudwatch:namespace"
      values   = ["ER/Prod"]
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
