# The prod workload pods' AWS identities (§9.3, Pod Identity per the house rule
# — never IRSA, and there is no node-credential fallback to mask a mistake).
#
# Two roles, because there are two ServiceAccounts: `er-api` (the merged
# web+api pod, plus the dispatcher) and `er-runner` (one Kubernetes Job per
# task attempt). An EKS Pod Identity association binds ONE
# (cluster, namespace, service account) triple to ONE role, so every capability
# a pod needs has to land on that pod's single role — which is why SES and
# Bedrock contribute policies to `er-api` rather than defining roles of their
# own. envs/dev/pod_identity.tf carries the same note and the same shape.
#
# The difference from dev is the namespace list. Dev's two
# `*_pod_namespaces` variables default to `[]` because a dev namespace is
# created per developer by `scripts/dev_env.sh`, not by Terraform, and Pod
# Identity has no wildcard — so the list cannot be known. Prod has exactly one
# namespace, named by `local.app_namespace` in secrets.tf, so it is a literal
# here with no variable to set.

data "aws_caller_identity" "current" {}

data "aws_iam_policy_document" "pod_trust" {
  statement {
    effect  = "Allow"
    actions = ["sts:AssumeRole", "sts:TagSession"]

    principals {
      type        = "Service"
      identifiers = ["pods.eks.amazonaws.com"]
    }
  }
}

# --- er-api: the web+api pod and the dispatcher ------------------------------

resource "aws_iam_role" "er_api_pod" {
  name               = "er-prod-api-pod"
  assume_role_policy = data.aws_iam_policy_document.pod_trust.json

  tags = {
    Name = "er-prod-api-pod"
  }
}

resource "aws_eks_pod_identity_association" "er_api" {
  cluster_name    = module.eks.cluster_name
  namespace       = local.app_namespace
  service_account = "er-api"
  role_arn        = aws_iam_role.er_api_pod.arn
}

# This role's policy set is intentionally minimal, and the association above
# is the part that had to exist first: it is what makes the ServiceAccount's
# credential resolvable at all, and an absent association is a pod that fails
# to start with a credential error that reads like a bug. Capabilities then
# land as POLICIES on this one role, because Pod Identity binds one role per
# (cluster, namespace, ServiceAccount) triple. The first is ses.tf's
# send-transactional-mail; Bedrock remains unattached, there being no prod
# enablement.
#
# The dispatcher shares this ServiceAccount (deployment-dispatcher.yaml) and
# needs no AWS access of its own — it talks to Postgres and the Kubernetes API.

# --- er-runner: one Job per task attempt -------------------------------------

resource "aws_iam_role" "er_runner_pod" {
  name               = "er-prod-runner-pod"
  assume_role_policy = data.aws_iam_policy_document.pod_trust.json

  tags = {
    Name = "er-prod-runner-pod"
  }
}

# The lake grants (lake.tf). In prod this role is the ONLY thing that has ever
# held them — dev reached the same document through a static IAM-user key for
# its first phase and retired it at 4.1, which is the history prod skips.
#
# Deliberately ahead of its first use: the engine cannot open an `s3://` lake
# today (the ducklake-lakehouse skill records the gap), so the prod data path
# is EFS at /srv/er and these grants go unexercised until that lands. The
# association, not the policy, is the part that must exist first.
resource "aws_iam_role_policy" "er_runner_lake" {
  name   = "er-prod-runner-lake"
  role   = aws_iam_role.er_runner_pod.id
  policy = data.aws_iam_policy_document.lake_access.json
}

resource "aws_eks_pod_identity_association" "er_runner" {
  cluster_name    = module.eks.cluster_name
  namespace       = local.app_namespace
  service_account = "er-runner"
  role_arn        = aws_iam_role.er_runner_pod.arn
}

output "er_api_pod_role_arn" {
  description = "The role the er-api ServiceAccount assumes via Pod Identity."
  value       = aws_iam_role.er_api_pod.arn
}

output "er_runner_pod_role_arn" {
  description = "The role the er-runner ServiceAccount assumes via Pod Identity."
  value       = aws_iam_role.er_runner_pod.arn
}
