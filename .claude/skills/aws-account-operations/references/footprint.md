# What the account holds

## Contents

- Why this file is generated
- How to read it
- Resource types declared
- Identity Center permission sets
- The AWS SDK surface in the Rust workspace
- Terraform and provider pins
- Where the engine's AWS access actually comes from

## Why this file is generated

Generated from source by `scripts/skills-check.sh regen` — resource types and
counts are read out of `infra/terraform/`, the permission sets out of
`modules/identity-center/main.tf`, the AWS SDK surface out of the workspace
`Cargo.toml` files, and the version pins out of the `versions.tf` files. **Do
not hand-edit below the marker.**

Everything here is a count, a name or a version, which is the category that goes
stale fastest. The reasoning that explains *why* the account looks like this
lives in the skill body and in `docs/infrastructure.md`; this file is the
inventory.

## How to read it

**Declared, not observed.** This is what the Terraform says, which is not
necessarily what is running — `terraform plan` in CI is still a roadmap gap, and
the `envs/prod` stack has never been applied. For what is actually live, a
read-only `aws describe`/`list` call is the answer today; see the skill body for
the profile to use.

**A resource type absent from the list is genuinely absent**, and that is often
the useful fact. There is no CloudFront, ACM, Route 53, WAF, Athena, Glue,
DynamoDB, Lambda, SQS, SNS, GuardDuty or Security Hub in this account. So:
TLS termination and DNS are Cloudflare's; there is no edge cache; there is no
queue service, because the job queue is Postgres `SKIP LOCKED`; and there is no
alerting path beyond budget email.

**Counts are blocks, not instances.** A module instantiated twice (the lake and
EFS modules are) contributes its resource blocks once to this count while
creating two sets of real resources.

<!-- BEGIN GENERATED: aws-footprint -->
### Resource types declared

Distinct `resource "aws_*"` blocks across `infra/terraform/`, by count:

```
   9 aws_iam_role
   6 aws_iam_role_policy
   5 aws_s3_bucket_public_access_block
   5 aws_s3_bucket_policy
   5 aws_s3_bucket
   4 aws_ssoadmin_permission_set
   4 aws_ssoadmin_account_assignment
   4 aws_secretsmanager_secret_version
   4 aws_secretsmanager_secret
   4 aws_kms_key
   4 aws_kms_alias
   4 aws_iam_role_policy_attachment
   4 aws_eks_pod_identity_association
   4 aws_eks_addon
   3 aws_ssoadmin_managed_policy_attachment
   3 aws_security_group
   3 aws_s3_bucket_versioning
   3 aws_s3_bucket_server_side_encryption_configuration
   2 aws_subnet
   2 aws_ssoadmin_permission_set_inline_policy
   2 aws_route_table_association
   2 aws_route_table
   2 aws_route
   2 aws_rds_cluster_instance
   2 aws_rds_cluster
   2 aws_identitystore_group_membership
   2 aws_identitystore_group
   2 aws_iam_user_policy
   2 aws_iam_user
   2 aws_iam_access_key
   2 aws_db_subnet_group
   2 aws_cloudwatch_log_group
   2 aws_budgets_budget
   1 aws_vpc_endpoint
   1 aws_vpc
   1 aws_sesv2_email_identity_mail_from_attributes
   1 aws_sesv2_email_identity
   1 aws_sesv2_configuration_set
   1 aws_s3_bucket_object_lock_configuration
   1 aws_s3_bucket_lifecycle_configuration
   1 aws_organizations_organization
   1 aws_organizations_account
   1 aws_nat_gateway
   1 aws_internet_gateway
   1 aws_iam_user_policy_attachment
   1 aws_iam_service_linked_role
   1 aws_iam_openid_connect_provider
   1 aws_eks_cluster
   1 aws_eks_access_policy_association
   1 aws_eks_access_entry
   1 aws_eip
   1 aws_efs_mount_target
   1 aws_efs_file_system
   1 aws_efs_backup_policy
   1 aws_ecr_repository
   1 aws_ecr_lifecycle_policy
   1 aws_config_delivery_channel
   1 aws_config_configuration_recorder_status
   1 aws_config_configuration_recorder
   1 aws_cloudtrail
   1 aws_bedrock_use_case_for_model_access
   1 aws_bedrock_foundation_model_agreement
   1 aws_bcmdataexports_export
```

### Identity Center permission sets

| Permission set | Session | Inline policy |
|---|---|---|
| `PlatformAdmin` | PT8H | yes |
| `PlatformReadOnly` | PT8H | **none** |
| `ProdDataReadOnly` | PT4H | yes |
| `ProdAdmin` | PT1H | **none** |

### The AWS SDK surface in the Rust workspace

Every `aws-*` dependency, with the crate that declares it. All must stay
`default-features = false`; the reasons are in `bedrock-and-sdks.md`.

| Crate | Declared by |
|---|---|
| `aws-config` | `er-server` |
| `aws-sdk-sesv2` | `er-server` |
| `aws-smithy-http-client` | `er-server` |

Any crate above whose declaration is missing `default-features = false`
will fail the image build — `aws-lc-sys` needs a `cmake` the build image
does not carry:

```
(every aws-* declaration pins default-features = false)
```

### Terraform and provider pins

```
required_version = ">= 1.11"
source  = "hashicorp/aws"
source  = "hashicorp/random"
version = "~> 3.0"
version = "~> 6.0"
```

### Where the engine's AWS access actually comes from

Workload credentials, by mechanism — the thing to check first when a pod
cannot reach AWS:

| Mechanism | Count |
|---|---|
| `aws_eks_pod_identity_association` blocks | 4 |
| `aws_iam_user` blocks (should be the lake key and the builder key only) | 2 |
| `aws_iam_openid_connect_provider` blocks (GitHub Actions only; never for workloads) | 1 |
<!-- END GENERATED: aws-footprint -->
