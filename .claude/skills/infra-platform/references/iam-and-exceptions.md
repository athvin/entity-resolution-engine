# IAM: the convention, and the debt

## Contents

- The convention
- The builder credential
- Why the prod guard is Deny-based
- The one deliberate omission
- Why PlatformAdmin cannot create it
- The lake service user
- The retirement checklist

## The convention

Three rules cover almost every case:

1. **Workloads use EKS Pod Identity.** A role trusting
   `pods.eks.amazonaws.com`, bound by `aws_eks_pod_identity_association` to a
   namespace and ServiceAccount. No IRSA, no node-role credentials.
2. **Humans use Identity Center**, with permission sets and a session cap. No
   long-lived human access keys — the PlatformAdmin permission set enforces this
   on itself by denying `iam:CreateAccessKey`.
3. **CI uses GitHub OIDC**, with the role's `sub` pinned to pushes on the
   default branch so forks and pull requests cannot assume it.

Everything below is an exception to rule 2, and each one is tagged in code with
what retires it.

## The builder credential

`infra/terraform/envs/dev/builder.tf` creates a long-lived IAM **user** with
`AdministratorAccess` for the MVP build loop. It is flagged in its own banner
as temporary, dev-scoped and prod-guarded, and tagged:

```hcl
Retire      = "when-build-loop-done"
ExceptionTo = "infrastructure.md-8.1-no-longlived-human-keys"
```

Its key material lives in Secrets Manager; only the secret's ARN is a Terraform
output, never the key itself.

The reason it exists: Identity Center sessions cap at a few hours and need an
interactive browser login, which an unattended build loop cannot do.

## Why the prod guard is Deny-based

The user holds `AdministratorAccess` **minus** an inline policy of explicit
Denies. That ordering matters: in IAM an explicit Deny always wins over any
Allow, so the guard cannot be escaped by the admin grant, by a future policy
attachment, or by assuming a role the admin policy would otherwise permit.

The Denies cover mutation of production resources by name pattern and by tag:

| Statement | Blocks |
|---|---|
| `DenyProdS3Mutation` | writes to prod buckets |
| `DenyProdRdsMutation` | prod database mutation |
| `DenyProdEksMutation` | prod cluster mutation |
| `DenyProdSecretsMutation` | writes to `er/prod/*` secrets |
| `DenyProdKeyDeletion` | KMS key deletion, conditioned on the prod environment tag |
| `DenyProdRoleMinting` | creating roles on `er-prod-*` |

Building the guard as a denylist over admin is weaker in principle than an
allowlist — an unanticipated service is permitted. It is accepted here because
the credential is explicitly temporary and scoped to a dev account. Do not
reuse the pattern for anything intended to last.

## The one deliberate omission

`iam:CreateAccessKey` is **not** denied on the builder, unlike on
PlatformAdmin. The build loop must be able to mint the dev lake's access key.

This is the exception inside the exception, and it is the reason the builder
cannot be treated as "admin minus prod": it can also create credentials. Weigh
that when considering what else to let the loop do.

## Why PlatformAdmin cannot create it

PlatformAdmin's own guard denies `iam:CreateAccessKey`, so a PlatformAdmin
session **cannot apply the builder stack** — it cannot create the user's access
key.

Applying it requires the management profile:

```sh
AWS_PROFILE=er-mgmt terraform -chdir=infra/terraform/envs/dev apply
```

If that apply fails with an access-key denial, the guard is working. Do not
relax PlatformAdmin to make the apply convenient — that would remove the
control that makes rule 2 real.

## The lake service user

A second, narrower exception: a lake-scoped IAM user with keys in Secrets
Manager, consumed by the dev env script at bring-up. It exists because the Rust
engine has no object-store credential path yet (see the ducklake-lakehouse
skill), so there is nothing for Pod Identity to feed.

It retires when the engine reaches the credential chain — which makes it
blocked on application work, not on infrastructure work.

## The retirement checklist

Before the production gate, both exceptions must be gone. In order:

```
- [ ] 1. The engine reaches object storage through the credential chain
         (Pod Identity), so the lake service user has no consumer
- [ ] 2. Delete the lake service user and its Secrets Manager entry
- [ ] 3. Confirm nothing automated still needs AdministratorAccess
- [ ] 4. Delete the er-dev-builder user, its access key and its secret
- [ ] 5. Remove builder.tf and apply
- [ ] 6. Audit for self-granted cluster access entries added during the build
         loop and remove them
- [ ] 7. Re-confirm no long-lived human key exists in the account
```

Steps 1 and 2 are ordered: deleting the service user before the credential
chain works would break every dev lake. Step 6 is easy to forget because an
access entry added by hand leaves no Terraform diff.
