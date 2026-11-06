---
name: infra-platform
description: Explains how to author this repo's AWS and Kubernetes platform — the Terraform env/module layout and shared S3 state with lockfile locking, EKS Auto Mode (no node groups and no Karpenter installation, though its `karpenter.sh/v1` NodePool CRDs are how capacity is shaped), the EKS Pod Identity convention that replaces IRSA everywhere, the two Helm charts and the merged web-api pod, per-developer namespaces, and the deliberate IAM exceptions that must be retired before production. Use when editing Terraform or a Helm chart, writing a NodePool, bringing up or tearing down a dev environment, or reviewing an IAM policy. For operating or introspecting the live account — what is deployed, a bill, a quota error, the MCP tools and read-only guardrails — see aws-account-operations; for CI workflows and the image build see ci-and-supply-chain; for lake storage semantics see ducklake-lakehouse.
---

# The platform

Single AWS account, one region, two EKS clusters (`er-dev` and `er-prod`).
Terraform owns the cloud; Helm owns what runs in the cluster.

**Terraform is the only infrastructure language here — never author CDK,
CloudFormation or SAM.** There is no `cdk.json`, no `template.yaml` and no
`samconfig`, and that is a decision: a second app would be an unmanaged source
of truth for the same account. Reading CloudFormation is fine, since an EKS
addon can surface as a stack AWS manages for you; adding to one is not.

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
runs its own Karpenter.** The manifests in `infra/k8s/nodepools/` are literally
`apiVersion: karpenter.sh/v1`, `kind: NodePool`, and
`docs/infrastructure.md` §5.1 is titled "Karpenter disruption settings". So:

- Writing a `karpenter.sh/v1` `NodePool` is **correct** and is the supported way
  to declare instance families, capacity type and disruption policy.
- Installing Karpenter, or managing its controller or a custom `NodeClass`, is
  **wrong** — Auto Mode owns that.
- A new pool goes **beside the deployed ones**, not under
  `infra/k8s/nodepools/measure/`. That subdirectory holds a measurement-only
  pool that `make nodepools` deliberately does not apply, and it is the one
  `NodePool` in the tree carrying `limits`, so a runaway measurement cannot
  provision a fleet.

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
| `envs/prod/` | the production subset — no builder, no secrets, no SES, no observability stack |
| `bootstrap/organization/`, `bootstrap/state-backend/` | the two chicken-and-egg stacks |
| `modules/` | `eks`, `network`, `lake`, `efs`, `ses`, `identity-center`, `github-oidc` |

Every stack shares **one S3 backend** with **`use_lockfile = true` and no
DynamoDB table** — S3 conditional writes provide the lock. Do not add a
DynamoDB lock table; it is not how this is configured.

Each env has its own state key, so a dev apply cannot touch prod state. Prod
also has no builder credential and no secrets stack, which is deliberate.

## How Terraform reaches the account

`infra.yml` runs four tiers and **CI owns every apply**: `fmt + validate`
(credential-free, every stack) → `plan` per stack under the read-only
`er-ci-tf-plan` role → one sticky PR comment carrying the add/change/destroy
table and the diffs → `apply` on `main` only, unattended. It reads no stored
secret; every identity is a federated OIDC role. Do not run a
recurring `terraform apply` by hand — merge is the trigger.

Four roles, and which one does what is not uniform:

| Role | Reachable from | Does |
|---|---|---|
| `er-ci-tf-plan` | any ref, OIDC | plans, `-refresh=false` — ReadOnlyAccess + state read |
| `er-ci-tf-apply` | `main` only, via the `infra-apply-*` environments | applies `core`, `dev` |
| `er-ci-tf-apply-prod` | `er-ci-tf-apply`, or `ProdAdmin` break-glass | applies `prod` |
| `er-ci-tf-drift` | `drift.yml` on `main` only | `plan -refresh-only`, daily |

**Only `er-ci-tf-drift` may refresh**, and that is the whole reason it exists.
The plan tier runs `-refresh=false` because a refreshing plan reads secret
*values* and `er-ci-tf-plan` is assumable from any PR branch — which can edit
the workflow it runs under. The drift role holds the two reads `ReadOnlyAccess`
omits (`secretsmanager:GetSecretValue`, `bcm-data-exports:GetExport`), which is
safe only because its trust needs **two** conditions: `sub` pinned to
`refs/heads/main`, *and* `job_workflow_ref` pinned to `drift.yml`. The first
alone would admit any `main`-ref job including the plan tier on a push; the
second is what stops a step added elsewhere inheriting secret-read. If you ever
need a refreshing plan somewhere new, that is a new role, not a widened
condition.

Both conditions are **exact matches, never wildcards**, and that is enforced by
AWS rather than by taste: a condition on `sub` or `job_workflow_ref` whose
pattern *begins* with `*` makes `CreateRole` fail with
`MalformedPolicyDocument: … which is not scoped to all`, and an exact `sub`
match sitting beside it does not satisfy the validator. Where the claim's format
was uncertain, the fix was to list both candidate forms as literals — not to
wildcard the uncertain part.

`envs/prod` goes through the second hop because `er-ci-tf-apply` is **denied the
prod state object** — Terraform cannot apply a stack whose state it cannot read,
so one deny covers the stack without a list of prod resource types to maintain,
and the hop cannot quietly stop being used. That buys legibility (CloudTrail
records a distinct `AssumeRole` tagged with actor, commit and branch — which is
why the trust allows `sts:TagSession`, since chaining keeps the session tags
that the web-identity paths drop) and rules out an *accidental* prod apply from
the non-prod identity. It is **not** containment: the apply role may assume the
prod role by design.

Break-glass, when CI cannot do it: assume `ProdAdmin` through SSO, then
`aws sts assume-role --role-arn arn:aws:iam::797781631727:role/er-ci-tf-apply-prod`
and run Terraform with those credentials — the same identity and permissions CI
uses, rather than an improvised one.

Rules that are easy to break:

- **Never add a `paths:` filter to `infra.yml`.** The plan tiers are required
  status checks, and a workflow a path filter skips never reports, so the PR
  would block forever. The `detect` job plus an `if:` is how a non-infra PR
  skips — a job skipped by `if:` reports "skipped", which protection accepts.
- **Every matrix job sets `name:` explicitly.** The matrix is a list of
  `{key, dir}` objects, and left to itself GitHub names the job
  `plan (core, infra/terraform/envs/core)`, silently un-protecting `plan (core)`.
- **A new stack needs the plan role's state read first, in its own PR.** The
  role's policy enumerates `envs/*.tfstate` and `bootstrap/*.tfstate`; a PR
  adding both a grant and the matrix leg that needs it fails its own new leg,
  because the grant is only live after the merge applies it (#358, and #350
  before it).
- **`bootstrap/state-backend` is planned and never applied from CI** — it owns
  the bucket its own backend lives in. **`bootstrap/organization` is validated
  only**: its one required variable is a root account email deliberately kept
  out of version control. `terraform validate` needs no variable value, which is
  exactly why it can sit in that tier and not in the plan tier.
- **A saved plan file is sensitive.** The apply tier plans to `tfplan`, shows
  it, guards it and applies that file. `tfplan` must never become an artifact
  and `terraform show -json` must never be printed — both carry sensitive values
  in cleartext, while `show -no-color` redacts them to `(sensitive value)`.

The apply is unattended, so the only thing between a merge and a production
teardown is `scripts/ci/tf-destroy-guard.sh`: it refuses any apply with a
non-zero destroy count unless the commit message carries `ALLOW-DESTROY` (the
repo squash-merges, so that means the PR title). It is a speed bump, not a
policy engine — it cannot tell a benign replace from losing Aurora, and a
*replace* trips it too.

Because that guard matters more than anything else in the file and its refusal
path has never fired on a real apply, `apply` gates on a helpers self-test job
that asserts the refusal, the override and the fail-closed branch against
fixtures (`scripts/ci/tf-helpers-test.sh`, runnable locally, no AWS). Changing a
helper without updating that suite is how the guard silently stops guarding —
the suite is mutation-checked, so breaking a helper does turn it red.

Job display names rename without warning, so take them from
`ci-and-supply-chain`'s generated required-checks reference rather than from
here — including which of these tiers is the aggregator that protection
actually requires.

## The two Helm charts

**`er-platform`** — the application. Notable shapes:

- **One merged pod** runs the Rust control plane and the Next.js BFF. The
  dispatcher is **its own Deployment** (`er-dispatcher`, replicas 1/Recreate,
  batch-orchestration-design §6.7): it is the only identity with Job-create
  RBAC, so it must not sit on the pod serving the API behind the BFF. Kinds
  listed in `executor.dagKinds` run as per-attempt Kubernetes Jobs rendered
  from the chart's task-template ConfigMap; everything else still runs
  in-process inside the dispatcher pod — the legacy arm, which Phase 4 of the
  batch-orchestration plan retires.
  `ER_DISPATCH_IN_PROCESS=1` survives only in local dev (`make run`).
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
