# Prod lake bucket (SSE-KMS under its own CMK, TLS-only, versioned,
# prevent_destroy — same module as dev). Unlike envs/dev/lake.tf there is NO
# static IAM-user access key here: prod reaches S3 via EKS Pod Identity (an IAM
# role), wired with the app deploy. §8.1 forbids human-minted long-lived keys,
# and prod honours that from day one (the dev static key is the Phase-1 interim
# only).
module "lake" {
  source = "../../modules/lake"

  name = "er-prod-lake-797781631727"
}

output "lake_bucket" {
  value = module.lake.bucket
}

output "lake_bucket_arn" {
  value = module.lake.bucket_arn
}

output "lake_kms_key_arn" {
  value = module.lake.kms_key_arn
}
