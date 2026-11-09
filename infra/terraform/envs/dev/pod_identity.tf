# The `er-api` pod's AWS identity.
#
# §9.3 gives the web+api Deployment one ServiceAccount — `er-api`, shared by the
# `er-api` and `er-web` containers (`deployment-web-api.yaml`). An EKS Pod
# Identity association binds ONE (cluster, namespace, service account) triple to
# ONE role, so every capability that pod needs has to land on the same role.
# SES used to define its own `er-dev-mail` role and its own association; adding
# Bedrock the same way would have been a second association for the same triple,
# which is not a thing. So the role lives here and each capability contributes a
# policy to it.
#
# `er_api_pod_namespaces` is the SES `mail_pod_namespaces` generalised, for the
# same reason it was per-namespace before: a dev namespace is created per
# developer by `scripts/dev_env.sh`/Helm, not by Terraform, and Pod Identity has
# no wildcard. A developer who wants real AWS-backed mail or AI adds their
# namespace; everyone else runs `ER_MAIL_BACKEND=log` with AI disabled and needs
# no AWS at all.

variable "er_api_pod_namespaces" {
  description = <<-EOT
    Namespaces whose `er-api` ServiceAccount gets an AWS identity (SES send +
    Bedrock invoke, whichever are enabled).

    Empty by default, and that is the honest default — see the note above.
    Prod has one fixed namespace, so this becomes a one-element list there.
  EOT
  type        = list(string)
  default     = []
}

data "aws_iam_policy_document" "er_api_pod_trust" {
  statement {
    effect  = "Allow"
    actions = ["sts:AssumeRole", "sts:TagSession"]

    principals {
      type        = "Service"
      identifiers = ["pods.eks.amazonaws.com"]
    }
  }
}

resource "aws_iam_role" "er_api_pod" {
  name               = "er-dev-api-pod"
  assume_role_policy = data.aws_iam_policy_document.er_api_pod_trust.json

  tags = {
    Name        = "er-dev-api-pod"
    Environment = "dev"
  }
}

resource "aws_eks_pod_identity_association" "er_api" {
  for_each = toset(var.er_api_pod_namespaces)

  cluster_name    = module.eks.cluster_name
  namespace       = each.value
  service_account = "er-api"
  role_arn        = aws_iam_role.er_api_pod.arn
}

output "er_api_pod_role_arn" {
  description = "The role the er-api ServiceAccount assumes via Pod Identity."
  value       = aws_iam_role.er_api_pod.arn
}

# The fallback runner identity keeps existing associations stable, but has
# only GetBucketLocation. It cannot read, write or list lake objects. Registered
# orgs use the distinct identities in tenant_runners.tf; the chart and executor
# select them from the same registry and the authoritative task org.
# Local/EFS-only orgs need no AWS lake grants and keep the fallback account.

variable "er_runner_pod_namespaces" {
  description = <<-EOT
    Namespaces whose `er-runner` ServiceAccount gets an AWS identity (bucket
    location only; tenant lake access is registered in tenant_runners.tf).

    Empty by default, like `er_api_pod_namespaces` and for the same reason.
  EOT
  type        = list(string)
  default     = []
}

resource "aws_iam_role" "er_runner_pod" {
  name               = "er-dev-runner-pod"
  assume_role_policy = data.aws_iam_policy_document.er_api_pod_trust.json

  tags = {
    Name        = "er-dev-runner-pod"
    Environment = "dev"
  }
}

# Retain the policy address while removing all bucket-wide data grants.
resource "aws_iam_role_policy" "er_runner_lake" {
  name   = "er-dev-runner-lake"
  role   = aws_iam_role.er_runner_pod.id
  policy = data.aws_iam_policy_document.lake_access.json
}

resource "aws_eks_pod_identity_association" "er_runner" {
  for_each = toset(var.er_runner_pod_namespaces)

  cluster_name    = module.eks.cluster_name
  namespace       = each.value
  service_account = "er-runner"
  role_arn        = aws_iam_role.er_runner_pod.arn
}

output "er_runner_pod_role_arn" {
  description = "The role the er-runner ServiceAccount assumes via Pod Identity."
  value       = aws_iam_role.er_runner_pod.arn
}
