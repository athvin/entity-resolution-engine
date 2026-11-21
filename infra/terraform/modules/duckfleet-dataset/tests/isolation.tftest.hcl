mock_provider "aws" {}
variables {
  environment  = "dev"
  region       = "us-east-2"
  cluster_name = "er-dev"
  cluster_arn  = "arn:aws:eks:us-east-2:123456789012:cluster/er-dev"
  bucket_arn   = "arn:aws:s3:::test-lake"
  kms_key_arn  = "arn:aws:kms:us-east-2:123456789012:key/test"
  datasets = [
    { org_id = "org-a", dataset_id = "dataset-a" },
    { org_id = "org-a", dataset_id = "dataset-b" },
    { org_id = "org-b", dataset_id = "dataset-c" }
  ]
}
run "dataset_boundaries" {
  command = plan
  assert {
    condition     = output.namespaces["dataset-a"] == "duckfleet-2c92e260fe9935ecd6664c5a84639cd7b464e280"
    error_message = "Namespace recipe must match the Rust binding resolver."
  }
  assert {
    condition = length(distinct(values(output.namespaces))) == 3 && alltrue([
      for id, ns in output.namespaces :
      jsondecode(output.policies[id]).Statement[0].Resource == ["${var.bucket_arn}/duckfleet/${ns}/*"] &&
      jsondecode(output.policies[id]).Statement[1].Condition.StringLike["s3:prefix"] == ["duckfleet/${ns}/*"] &&
      output.service_accounts[id] == "duckfleet-worker"
    ])
    error_message = "Same-org and cross-org datasets must have disjoint object/list grants and the upstream worker service account."
  }
}
run "invalid_identity_refused" {
  command = plan
  variables { datasets = [{ org_id = "org-a", dataset_id = "../other" }] }
  expect_failures = [var.datasets]
}
run "workers_cannot_seal_or_delete_accepted_artifacts" {
  command = plan
  assert {
    condition = alltrue([for id, ns in output.namespaces :
      jsondecode(output.policies[id]).Statement[4].Effect == "Deny" &&
      jsondecode(output.policies[id]).Statement[4].Resource == ["${var.bucket_arn}/duckfleet/${ns}/artifacts/immutable/*"] &&
      toset(jsondecode(output.policies[id]).Statement[4].Action) == toset(["s3:PutObject", "s3:DeleteObject", "s3:DeleteObjectVersion", "s3:AbortMultipartUpload"])
    ])
    error_message = "Workers may read sealed artifacts but must never mutate or delete them; staging stays outside this deny."
  }
}
