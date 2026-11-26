mock_provider "aws" {}

variables {
  environment  = "dev"
  region       = "us-east-2"
  cluster_name = "er-dev"
  cluster_arn  = "arn:aws:eks:us-east-2:123456789012:cluster/er-dev"
  bucket_arn   = "arn:aws:s3:::test-lake"
  kms_key_arn  = "arn:aws:kms:us-east-2:123456789012:key/test"
  bindings = {
    a = { namespace = "ci", org_id = "tenant-a", prefix = "tenant-a/" }
    b = { namespace = "ci", org_id = "tenant-b", prefix = "tenant-b/" }
  }
}

run "tenant_boundaries" {
  command = plan
  assert {
    condition     = jsondecode(output.policies["a"]).Statement[0].Resource == ["arn:aws:s3:::test-lake/tenant-a/*"]
    error_message = "Tenant A must have object access only below its slash-terminated prefix."
  }
  assert {
    condition     = jsondecode(output.policies["a"]).Statement[1].Condition.StringLike["s3:prefix"] == ["tenant-a/*"]
    error_message = "Root and other-tenant listings must not inherit the bucket grant."
  }
  assert {
    condition     = output.service_accounts["a"] != output.service_accounts["b"] && output.service_accounts["a"] == "er-runner-${substr(sha256("tenant-a"), 0, 20)}"
    error_message = "Distinct orgs must match the Helm/runtime ServiceAccount recipe."
  }
  assert {
    condition = jsondecode(aws_iam_role.tenant["a"].assume_role_policy).Statement[0].Condition.StringEquals == {
      "aws:RequestTag/eks-cluster-arn"            = var.cluster_arn
      "aws:RequestTag/kubernetes-namespace"       = "ci"
      "aws:RequestTag/kubernetes-service-account" = output.service_accounts["a"]
    }
    error_message = "Pod Identity trust must bind the whole cluster/namespace/account triple."
  }
  assert {
    condition     = jsondecode(output.policies["a"]).Statement[4].Condition.StringEquals["kms:ViaService"] == "s3.us-east-2.amazonaws.com"
    error_message = "A runner must not get direct KMS decryption."
  }
}

run "root_refused" {
  command = plan
  variables { bindings = { a = { namespace = "ci", org_id = "tenant-a", prefix = "/" } } }
  expect_failures = [var.bindings]
}

run "wildcard_refused" {
  command = plan
  variables { bindings = { a = { namespace = "ci", org_id = "tenant-a", prefix = "tenant-*/" } } }
  expect_failures = [var.bindings]
}

run "traversal_refused" {
  command = plan
  variables { bindings = { a = { namespace = "ci", org_id = "tenant-a", prefix = "a/../b/" } } }
  expect_failures = [var.bindings]
}

run "overlap_refused" {
  command = plan
  variables {
    bindings = {
      a = { namespace = "ci", org_id = "tenant-a", prefix = "a/" }
      b = { namespace = "other", org_id = "tenant-b", prefix = "a/b/" }
    }
  }
  expect_failures = [var.bindings]
}

run "duplicate_identity_refused" {
  command = plan
  variables {
    bindings = {
      a = { namespace = "ci", org_id = "tenant-a", prefix = "a/" }
      b = { namespace = "ci", org_id = "tenant-a", prefix = "b/" }
    }
  }
  expect_failures = [var.bindings]
}
run "foreign_read_only_prefix_refused" {
  command = plan
  variables {
    bindings = {
      a = { namespace = "ci", org_id = "tenant-a", prefix = "a/", read_only_prefixes = ["b/immutable/"] }
    }
  }
  expect_failures = [var.bindings]
}
run "wildcard_read_only_prefix_refused" {
  command = plan
  variables {
    bindings = {
      a = { namespace = "ci", org_id = "tenant-a", prefix = "a/", read_only_prefixes = ["a/*/"] }
    }
  }
  expect_failures = [var.bindings]
}

run "export_readers" {
  command = plan
  variables { export_reader_principal_arn = "arn:aws:iam::123456789012:role/er-dev-api-pod" }
  assert {
    condition     = jsondecode(output.export_policies["a"]).Statement[0].Resource == ["arn:aws:s3:::test-lake/tenant-a/exports/*"]
    error_message = "An export reader must read only its own tenant's exports/, never the lake data or another tenant."
  }
  assert {
    condition     = jsondecode(output.export_policies["a"]).Statement[0].Action == ["s3:GetObject"]
    error_message = "An export reader is read-only."
  }
  assert {
    condition     = aws_iam_role.export["a"].name == "er-dev-export-${substr(sha256("a"), 0, 20)}"
    error_message = "The reader's name must follow the recipe the chart derives ARNs from."
  }
  assert {
    condition     = jsondecode(aws_iam_role.export["a"].assume_role_policy).Statement[0].Principal.AWS == var.export_reader_principal_arn
    error_message = "Only the API pod role may assume an export reader."
  }
  assert {
    condition     = jsondecode(output.policies["a"]).Statement[2].Resource == ["arn:aws:s3:::test-lake/tenant-a/exports/*"]
    error_message = "The runner may tag only its own tenant's export artifacts."
  }
}

run "no_api_identity_no_readers" {
  command = plan
  assert {
    condition     = length(aws_iam_role.export) == 0
    error_message = "Without an API principal, no export reader exists."
  }
}
