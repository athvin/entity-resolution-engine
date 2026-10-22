---
name: infra-platform
description: Explains how to author this repo's AWS and Kubernetes platform — the Terraform env/module layout and shared S3 state with lockfile locking, EKS Auto Mode (no node groups and no Karpenter installation, though its `karpenter.sh/v1` NodePool CRDs are how capacity is shaped), the EKS Pod Identity convention that replaces IRSA everywhere, the two Helm charts and the merged web-api pod, per-developer namespaces, and the deliberate IAM exceptions that must be retired before production. Use when editing Terraform or a Helm chart, writing a NodePool, bringing up or tearing down a dev environment, or reviewing an IAM policy. For operating or introspecting the live account — what is deployed, a bill, a quota error, the MCP tools and read-only guardrails — see aws-account-operations; for CI workflows and the image build see ci-and-supply-chain; for lake storage semantics see ducklake-lakehouse.
---

# The platform

Single AWS account, one region, two EKS clusters (`er-dev` and `er-prod`).
Terraform owns the cloud; Helm owns what runs in the cluster.

## Workload AWS access is Pod Identity, never IRSA

This is the convention most likely to be got wrong, because almost every
external example uses IRSA.

```hcl
# trust
identifiers = ["pods.eks.amazonaws.com"]
# bind
resource "aws_eks_pod_identity_association" "…" { … }
```

No IRSA annotations, no OIDC provider for workloads, **and no node-role
credentials** — the IMDS hop limit makes the node path unreachable from a pod
anyway, so falling back to it is not an option that quietly works.

The only OIDC provider in the account is for **GitHub Actions → ECR push**, and
it is pinned to pushes on the default branch so forks and PRs cannot assume it.

So when a pod needs AWS access, the work is: a role trusting
`pods.eks.amazonaws.com`, plus an association binding it to that namespace and
ServiceAccount. If you find yourself adding an
`eks.amazonaws.com/role-arn` annotation, that is the IRSA pattern and it will
not work here.

## EKS Auto Mode — there are no node groups

The clusters run **Auto Mode**: `compute_config` enabled with the `system` node
pool, EKS-managed load balancing and block storage, and
`bootstrap_self_managed_addons = false`.

Consequences worth stating, because generic Kubernetes guidance assumes
otherwise:

- **No self-managed node groups** to size, patch or drain.
- **No Karpenter *installation*.** Do not add the Karpenter controller, a Helm
  release for it, or a `NodeClass`/provisioner of your own; capacity is EKS's
  job.
- Load balancing and EBS come from the cluster's own configuration rather than
  separately installed controllers.

**Karpenter CRDs are nevertheless how you shape capacity here, because Auto Mode
runs its own Karpenter.** The three manifests in `infra/k8s/nodepools/` are
literally `apiVersion: karpenter.sh/v1`, `kind: NodePool`, and
`docs/infrastructure.md` §5.1 is titled "Karpenter disruption settings". So:

- Writing a `karpenter.sh/v1` `NodePool` is **correct** and is the supported way
  to declare instance families, capacity type and disruption policy.
- Installing Karpenter, or managing its controller or a custom `NodeClass`, is
  **wrong** — Auto Mode owns that.

The distinction matters for disruption in particular:
`WhenEmptyOrUnderutilized` is right for the stateless app pool and forbidden on
the runner pools, because an app pod is a replaceable replica and a runner pod
is an hour of work.

Cluster access is granted through `aws_eks_access_entry` with an EKS access
policy, not by editing the `aws-auth` ConfigMap.

## Terraform layout and state

| Path | Holds |
|---|---|
| `envs/core/` | account-wide: CloudTrail, budgets, Config, cost reports, Identity Center, the one ECR registry, the GitHub OIDC role |
| `envs/dev/` | the dev VPC, EKS, RDS, lake bucket, EFS, secrets, SES, observability, and the builder credential |
| `envs/prod/` | the production subset — no builder, no secrets, no SES, no observability yet |
| `bootstrap/organization/`, `bootstrap/state-backend/` | the two chicken-and-egg stacks |
| `modules/` | `eks`, `network`, `lake`, `efs`, `ses`, `identity-center`, `github-oidc` |

All five stacks share **one S3 backend** with **`use_lockfile = true` and no
DynamoDB table** — S3 conditional writes provide the lock. Do not add a
DynamoDB lock table; it is not how this is configured.

Each env has its own state key, so a dev apply cannot touch prod state. Prod
also has no builder credential and no secrets stack, which is deliberate.

`terraform plan` in CI is **not built yet** (it is a known roadmap gap). The
infra workflow only does `fmt -check` and a credential-free `validate`, and it
is *not* a required check — which is why a real `paths:` filter is fine there,
unlike the frontend workflow.

## The two Helm charts

**`er-platform`** — the application. Notable shapes:

- **One merged pod** runs both the Rust control plane and the Next.js BFF, with
  the dispatcher **in-process** (`ER_DISPATCH_IN_PROCESS=1`). There is no
  separate dispatcher Deployment and there are no per-run Kubernetes jobs.
- Only the web service is exposed; the API service has no load balancer in any
  environment. The browser reaches the BFF, and only the BFF reaches the API —
  the same boundary the frontend-bff skill describes, enforced here by topology.
- Secrets are projected by External Secrets from Secrets Manager.
- **Drizzle migrations run as a pre-install/pre-upgrade hook** from the web
  image. So a schema change ships with the deploy that needs it, and a failed
  migration fails the release rather than leaving a half-migrated app serving.
- **The Rust schema reconciles in three places the chart owns.** An init
  container on the web-api pod runs `er-server migrate` (the control plane,
  before the new server serves — safe because the strategy is `Recreate`); a
  post-upgrade hook Job runs `er-server migrate-tenants` (every active
  tenant's lake, one line per tenant, non-zero on any tenant that is not
  current or migrated); and a daily CronJob runs the same sweep as drift
  detection. The last two are `tenantMigrate.*` values.
- The chart has **no `appVersion`**: images are pinned per install by tag, and
  prod promotes by digest.

**`er-dev-namespace`** — the per-developer substrate: a Pod Security
Standards-labelled namespace, an in-cluster Postgres StatefulSet with a raised
`max_locks_per_transaction`, a default-deny ingress NetworkPolicy, a
ResourceQuota and LimitRange, the namespace's ExternalSecret, and an
idle-suspend CronJob. `er-platform` installs on top of it.

Chart details and the dev lifecycle commands:
[references/clusters-and-charts.md](references/clusters-and-charts.md).

## The dev lifecycle

```sh
make dev-up DEV=<name> TAG=<git sha>
make dev-suspend DEV=<name>     # scale everything to zero
make dev-resume  DEV=<name>
make dev-down    DEV=<name>
```

`dev-resume` uses `kubectl scale`, **not** `helm upgrade`. After a `kubectl
scale` to zero, Helm's three-way merge sees no manifest difference and would do
nothing — so resuming through Helm silently leaves the environment down. Keep
the asymmetry.

## The IAM exceptions, and that they are temporary

Two deliberate, documented exceptions exist in dev and **must be retired before
the production gate**:

- **`er-dev-builder`** — a long-lived IAM user with `AdministratorAccess`, for
  the build loop. It is constrained by an inline prod-guard policy whose explicit
  Denies override the admin grant on every `er-prod-*` resource and `er/prod/*`
  secret. One omission is deliberate: `iam:CreateAccessKey` is **not** denied,
  because the loop must mint the dev lake's key. It is tagged with its own
  retirement condition.
- **A lake-scoped service user** — an interim credential for S3 access, to be
  retired when the engine reaches the credential chain.

A detail that trips people up: **`er-dev-builder` cannot be created by
PlatformAdmin**, because PlatformAdmin's own guard denies
`iam:CreateAccessKey`. Applying that stack requires the management profile.
That is the guard working, not a misconfiguration.

Treat both as scheduled debt. The policy rationale and the retirement checklist:
[references/iam-and-exceptions.md](references/iam-and-exceptions.md).

## Common workflows

**A workload needs to read a bucket or a secret.** A role trusting
`pods.eks.amazonaws.com`, an `aws_eks_pod_identity_association` for its
namespace and ServiceAccount, and a least-privilege policy. Not IRSA, not the
node role.

**A pod gets credential errors.** Check the association exists for that exact
namespace/ServiceAccount pair, and that the ServiceAccount name matches. There
is no node-credential fallback to mask a mistake, which is a feature.

**Changing the database schema the BFF owns.** It ships through the chart's
migrate hook, so the chart and the image version must agree.

**A release's tenant sweep failed.** `kubectl logs job/er-tenant-migrate`
is the per-tenant report. `breaking` means that tenant's lake needs a rebuild
(the detail names the relation and column); `timeout` means a job held the
lake past `tenantMigrate.timeoutSeconds` and that job's own start already
reconciled it; `error` is the lake not opening. `public.tenant_schema_states`
holds the same lines.

**Adding capacity.** There are no node groups. Describe the workload's
requirements and let Auto Mode place it.
