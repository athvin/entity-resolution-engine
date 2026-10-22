# Clusters, charts, and the dev lifecycle

## Contents

- Module responsibilities
- The `er-platform` templates
- The merged pod, and why
- EFS and the static PV
- `er-dev-namespace`
- The dev lifecycle, step by step
- Observability
- Non-chart manifests

## Module responsibilities

| Module | Owns |
|---|---|
| `eks` | the cluster: Auto Mode compute, EKS-managed load balancing and block storage, access entries |
| `network` | one VPC per env, three AZs, public + private subnets, and a **mandatory S3 Gateway endpoint** (no interface endpoints) |
| `lake` | the env's lake bucket: SSE-KMS with its own CMK, TLS-only, versioned with noncurrent and incomplete-upload expiry |
| `efs` | one filesystem per env, an access point per namespace, its own CMK |
| `ses` | domain identity, Easy DKIM, custom MAIL FROM, a configuration set — the SES **API** transport, not SMTP |
| `identity-center` | the permission sets, including the guard policy that denies long-lived key creation |
| `github-oidc` | the provider and the CI image-push role, pinned to default-branch pushes |

The S3 Gateway endpoint is load-bearing for lake traffic: it keeps bucket
access on the AWS network rather than through a NAT gateway, which matters for
both cost and egress policy.

## The `er-platform` templates

| Template | Deploys |
|---|---|
| `deployment-web-api.yaml` | the one merged pod (control plane + BFF, dispatcher in-process) |
| `services.yaml` | both services as ClusterIP — the API gets **no** load balancer in any env |
| `ingress-web.yaml` | flag-gated, **web only**, a shared internal ALB via a group name, DNS through external-dns |
| `externalsecrets.yaml` | External Secrets projections of the Secrets Manager entries |
| `efs.yaml` | the static PV for the shared filesystem |
| `job-db-migrate.yaml` | drizzle migrations as a pre-install/pre-upgrade hook |
| `serviceaccounts.yaml` | a single ServiceAccount, with `automountServiceAccountToken: false` |
| `instrumentation.yaml` | OpenTelemetry operator auto-instrumentation |

Two details worth preserving:

- **`automountServiceAccountToken: false`.** The pod does not talk to the
  Kubernetes API, so it does not get a token. AWS access comes from Pod
  Identity, which does not need one.
- **The chart has no `appVersion`.** Images are pinned per install (`--set
  images.tag=<sha>`), and prod promotes by **digest** rather than tag. So the
  chart version and the application version are deliberately decoupled.

## The merged pod, and why

One Deployment runs the Rust control plane on its port and the Next.js BFF on
its own, with the dispatcher in-process via `ER_DISPATCH_IN_PROCESS=1`.

What follows from it:

- **The topology enforces the BFF boundary.** The API service has no load
  balancer anywhere, so the only route to the control plane is through the BFF.
  That is the frontend-bff skill's rule, made physical.
- **No per-run Kubernetes jobs.** The dispatcher runs pipeline work in-process
  on a blocking thread. Scaling the pod scales both tiers together, which is a
  known trade — it is one deployable, not a microservice split.
- A dispatcher crash takes the API with it only if it is fatal; the in-process
  mode logs and keeps serving. See the er-server-control-plane skill.

## EFS and the static PV

The PV uses a `volumeHandle` of the form `<fs-id>::<access-point-id>` and a
fixed POSIX uid/gid. Both the filesystem id and the access point id are passed
in at install time (`--set efs.fileSystemId=… --set efs.accessPointId=…`),
resolved by the dev env script from Terraform outputs.

The EFS CSI addon gets its AWS access through a Pod Identity association
declared **inside** the addon resource, not as a separate role binding.

## `er-dev-namespace`

The per-developer substrate, installed into `default` and creating the
developer's namespace:

- A **Pod Security Standards-labelled** namespace.
- An in-cluster `postgres` StatefulSet plus PVC, started with a raised
  `max_locks_per_transaction` — the entity framework's DDL replay and history
  triggers take many locks in one transaction, and the default is too low.
- A **default-deny ingress** NetworkPolicy.
- A ResourceQuota and LimitRange, so one developer cannot starve the cluster.
- The namespace's ExternalSecret for its Postgres credentials.
- An **idle-suspend CronJob**, which is why a dev environment left alone scales
  itself down.

## The dev lifecycle, step by step

```sh
make dev-up DEV=<name> TAG=<git sha>
```

Both variables are required and the target refuses without them, because CI
pushes images **by commit sha** — there is no `latest`. Optional
`API_DIGEST`/`WEB_DIGEST` pin by digest instead.

What it does, in order:

1. Applies the gp3 StorageClass.
2. Evaluates `scripts/dev_env.sh up <dev>`, which resolves the EFS ids and
   other env from Terraform outputs.
3. Installs `er-dev-namespace` into `default` and waits.
4. Installs `er-platform` into the developer's namespace with the image tag and
   EFS ids, and waits.

Then:

| Command | Effect |
|---|---|
| `make dev-suspend DEV=…` | `kubectl scale deployment --all --replicas=0` |
| `make dev-resume DEV=…` | `kubectl scale` back up, then `rollout status` |
| `make dev-down DEV=…` | uninstall both releases, wait for namespace deletion, tear down the env |

**`dev-resume` is `kubectl`, not `helm`, on purpose.** After a `kubectl scale`
to zero the manifests are unchanged, so Helm's three-way merge computes no
diff and a `helm upgrade` would leave the environment down while reporting
success.

## Observability

`make obs-up` installs Fluent Bit and the OpenTelemetry collector and operator
from upstream charts at pinned versions, into their own namespace. Both get AWS
access by Pod Identity association — the Fluent Bit values explicitly note the
absence of an IRSA annotation, because that is the thing a reader would expect
to find.

Logs and metrics land in per-environment CloudWatch log groups with a finite
retention.

## Non-chart manifests

| Path | Purpose |
|---|---|
| `infra/k8s/nodepools/` | workload placement pools (not Karpenter) |
| `infra/k8s/observability/` | the namespace and the three upstream chart value files |
| `infra/k8s/secrets/` | the External Secrets ClusterSecretStore |
| `infra/k8s/storage/` | the gp3 StorageClass |
| `infra/k8s/network-policy-controller.yaml` | the policy controller |

These are applied directly rather than through a chart because they are
cluster-scoped and installed once, not per release.
