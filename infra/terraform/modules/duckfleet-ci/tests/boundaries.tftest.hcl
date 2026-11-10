mock_provider "aws" {}
variables {
  cluster_arn       = "arn:aws:eks:us-east-2:797781631727:cluster/er-dev"
  oidc_provider_arn = "arn:aws:iam::797781631727:oidc-provider/token.actions.githubusercontent.com"
}
run "separate_product_boundaries" {
  command = plan
  assert {
    condition = jsondecode(aws_iam_role.ci.assume_role_policy).Statement[0].Condition.StringEquals == {
      "token.actions.githubusercontent.com:aud" = "sts.amazonaws.com"
      "token.actions.githubusercontent.com:sub" = "repo:athvin@273880343/duck-fleet@1452020329:ref:refs/heads/main"
    }
    error_message = "Only immutable DuckFleet main may assume this role; no PR or other-repository subjects."
  }
  assert {
    condition     = aws_eks_access_entry.ci.kubernetes_groups == toset(["duckfleet:deploy"]) && aws_eks_access_entry.ci.cluster_name == "er-dev"
    error_message = "Use namespace RBAC on dev, never a system group or managed admin policy."
  }
  assert {
    condition = alltrue(flatten([for s in jsondecode(aws_iam_role_policy.ci.policy).Statement : [for a in s.Action :
      startswith(a, "s3:") || startswith(a, "ecr:") || a == "eks:DescribeCluster"
    ]]))
    error_message = "CI must not grant itself IAM, EKS writes or access to another AWS service."
  }
  assert {
    condition = alltrue([for s in jsondecode(aws_iam_role_policy.ci.policy).Statement :
      s.Resource != ["*"] || (s.Sid == "RegistryLogin" && s.Action == ["ecr:GetAuthorizationToken"])
    ])
    error_message = "Only ECR authentication may use a wildcard resource."
  }
  assert {
    condition = jsondecode(aws_iam_role_policy.ci.policy).Statement[1].Condition.StringLike["s3:prefix"] == ["duckfleet/*"] && jsondecode(aws_iam_role_policy.ci.policy).Statement[2].Resource == [
      "arn:aws:s3:::er-terraform-state-797781631727/duckfleet/dev.tfstate",
      "arn:aws:s3:::er-terraform-state-797781631727/duckfleet/dev.tfstate.tflock"
    ]
    error_message = "No read or write of ER state; lock deletion is separate from state writes."
  }
  assert {
    condition     = jsondecode(aws_iam_role_policy.ci.policy).Statement[3].Resource == ["arn:aws:s3:::er-terraform-state-797781631727/duckfleet/dev.tfstate.tflock"]
    error_message = "Only the lock object may be deleted by CI."
  }
  assert {
    condition = jsondecode(aws_iam_role_policy.ci.policy).Statement[4].Resource == [local.bucket_arn] && jsondecode(aws_iam_role_policy.ci.policy).Statement[6].Resource == [
      "arn:aws:ecr:us-east-2:797781631727:repository/duckfleet-dev/worker",
      "arn:aws:ecr:us-east-2:797781631727:repository/duckfleet-dev/dbt"
    ]
    error_message = "CI storage and image grants must name only the DuckFleet bucket/repositories."
  }
  assert {
    condition = jsondecode(aws_iam_role.worker.assume_role_policy).Statement[0].Condition.StringEquals == {
      "aws:RequestTag/eks-cluster-arn"            = var.cluster_arn
      "aws:RequestTag/kubernetes-namespace"       = "duckfleet-dev"
      "aws:RequestTag/kubernetes-service-account" = "duckfleet-worker"
    }
    error_message = "Worker trust must bind the complete Pod Identity tuple."
  }
  assert {
    condition     = jsondecode(aws_iam_role_policy.lake.policy).Statement[0].Resource == ["${local.bucket_arn}/lake/*"] && jsondecode(aws_iam_role_policy.lake.policy).Statement[1].Condition.StringLike["s3:prefix"] == ["lake/*"]
    error_message = "Worker object and list grants must stay under DuckFleet's lake prefix."
  }
  assert {
    condition     = aws_eks_pod_identity_association.worker.namespace == "duckfleet-dev" && aws_eks_pod_identity_association.worker.service_account == "duckfleet-worker" && !aws_eks_pod_identity_association.worker.disable_session_tags
    error_message = "Identity association and mandatory session tags must match trust."
  }
}
