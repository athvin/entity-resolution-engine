# Use the real policy-document provider function; mock resources only. A
# fabricated policy JSON would test the fixture instead of the generated deny.
mock_provider "aws" {
  mock_resource "aws_s3_bucket" {
    defaults = { arn = "arn:aws:s3:::test-lake" }
  }
}
provider "aws" {
  alias                       = "policy"
  region                      = "us-east-2"
  access_key                  = "testing"
  secret_key                  = "testing"
  skip_credentials_validation = true
  skip_requesting_account_id  = true
  skip_metadata_api_check     = true
}
variables {
  name               = "test-lake"
  immutable_prefixes = ["duckfleet/*/artifacts/immutable/"]
}
run "immutable_policy" {
  command = apply
  # aws_iam_policy_document is evaluated by the real provider without network.
  providers = { aws = aws.policy }
  override_resource {
    target = aws_s3_bucket.lake
    values = { arn = "arn:aws:s3:::test-lake", id = "test-lake" }
  }
  override_resource {
    target = aws_kms_key.lake
  }
  override_resource {
    target = aws_kms_alias.lake
  }
  override_resource {
    target = aws_s3_bucket_versioning.lake
  }
  override_resource {
    target = aws_s3_bucket_server_side_encryption_configuration.lake
  }
  override_resource {
    target = aws_s3_bucket_public_access_block.lake
  }
  override_resource {
    target = aws_s3_bucket_lifecycle_configuration.lake
  }
  override_resource {
    target = aws_s3_bucket_policy.lake
  }
  assert {
    condition = length([
      for s in jsondecode(data.aws_iam_policy_document.lake_tls_only.json).Statement : s
      if s.Sid == "RequireImmutableConditionalPublication" &&
      s.Effect == "Deny" && s.Action == "s3:PutObject" &&
      s.Resource == "arn:aws:s3:::test-lake/duckfleet/*/artifacts/immutable/*" &&
      try(s.Condition.Null["s3:if-none-match"], "") == "true"
    ]) == 1
    error_message = "Sealed artifacts must reject unconditional creation and overwrite."
  }
  assert {
    condition = length([
      for s in jsondecode(data.aws_iam_policy_document.lake_tls_only.json).Statement : s
      if s.Sid == "RetainImmutableArtifacts" && s.Effect == "Deny" &&
      try(toset(s.Action), toset([])) == toset(["s3:DeleteObject", "s3:DeleteObjectVersion"]) &&
      s.Resource == "arn:aws:s3:::test-lake/duckfleet/*/artifacts/immutable/*"
    ]) == 1
    error_message = "Deletion cannot make a content-addressed key replaceable."
  }
  assert {
    condition = alltrue([for rule in aws_s3_bucket_lifecycle_configuration.lake.rule :
      length(rule.expiration) == 0
    ])
    error_message = "Current accepted artifacts must not expire independently of delivery/result retention."
  }
}
