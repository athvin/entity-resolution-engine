# DuckFleet dataset isolation

## Contents

- Decision and cost
- Binding contract
- Provisioning and activation
- Rotation and revocation
- Deprovisioning and retention
- Evidence and release boundary

## Decision and cost

The controlled pilot uses **one DuckFleet deployment per dataset**, including
separate lake and platform databases, credentials, workers and query history.
Datasets in the same organization receive the same separation as datasets in
different organizations. Immutable org/dataset IDs derive a namespace:
`duckfleet-` plus the first 40 hex characters of SHA-256(`org_id + NUL + dataset_id`).
The NUL separator prevents ambiguous concatenations. Never use display names.

A shared deployment with server-controlled lake bindings was rejected for this
pilot: the pinned DuckFleet service trusts principals within a deployment and
exposes deployment-wide history. A SQL session also has the worker's storage
and catalog capabilities. Choosing a different catalog in SQL would not revoke
those capabilities. Namespace separation alone would not narrow storage IAM.

The selected model costs an idle gateway, persistent worker and catalog per
active dataset, plus catalog volume/backups and operational rollout overhead.
That cost grows with enrolled datasets even when no resolutions run. Suspend
compute only after draining sessions; retain catalogs and storage. No dollar
estimate is claimed from the development placeholder rate card. Measure idle
capacity, wake-up latency and cost per completed resolution in #1134/#667 before
expanding enrollment. A shared catalog service can reduce instance overhead
later only with dataset-specific credentials and proven access controls.

## Binding contract

`er-duckfleet` reads an operator-owned registry and projected credentials. No
HTTP DTO or job parameter accepts an endpoint, bucket, catalog, credential path
or worker identity. The caller first authorizes the org/dataset using the
existing control-plane guards and loads its immutable IDs. It then resolves
those IDs plus its server-recorded generation against a fresh registry snapshot.

Registry shape (synthetic example; no secret values):

```json
{
  "bucket": "test-lake",
  "bindings": [{
    "org_id": "org-a", "dataset_id": "dataset-a",
    "generation": 1, "state": "provisioning", "valid_until": 1800000000
  }]
}
```

Allowed states are `provisioning`, `active`, `draining`, `revoked`. Only an
unexpired `active` entry with the matching generation admits work. Missing
entries, malformed projections and duplicate dataset ownership fail closed;
there is no default fleet or local fallback. Reload before each admission,
not just once at process startup. An already admitted session is not revoked
by a file change; upstream credential revocation and draining remain required.

The gateway address is derived as `http://server.<namespace>.svc.cluster.local:8080`.
The namespace owns its private Postgres `lake` and `platform` databases, and
`duckfleet-worker` is its sole storage-capable ServiceAccount. Terraform's
`duckfleet-dataset` module grants `s3://<bucket>/duckfleet/<namespace>/` only;
`lake/` and `artifacts/` are children of that root. Pod Identity trust binds the
cluster, namespace and ServiceAccount, including the session tags. It reuses
the tenant-runner policy, not the broader org runner identity.

The secret projection is `<credential-mount>/<namespace>/credential.json`,
containing `org_id`, `dataset_id`, `generation` and `token`. It must match the
registry exactly. Reusing a token across active bindings refuses the entire
snapshot. Registry and credential types have no Debug/Serialize implementation;
parser errors never carry source input. Tokens belong only in authorization
headers. Do not put secrets in dataset/org env, queue parameters, SQL, metrics
or trace fields. The registry path and credential mount are process/operator
configuration, never request-controlled paths.

## Provisioning and activation

The `er-duckfleet-dataset` Helm boundary chart derives the same namespace as
the Rust resolver and Terraform module. It installs restricted pod security,
quota, default-deny ingress/egress and an HTTP-only gateway admission policy.
Only the named ER namespace can connect: its web-api pods, or task pods carrying
the server-assigned `er.dupezero.com/duckfleet-dataset` namespace label. The
upstream chart supplies namespace-local worker/catalog and scoped DNS/AWS
egress. These policies do not expose the catalog port to ER clients.

The platform owner reviews immutable enrollment IDs, applies Terraform IAM,
and owns the namespace boundary and secret lifecycle. DuckFleet's Helm release
owns the gateway, worker and private catalog. Use the pinned upstream namespace
provisioning structure, but override `lakePrefix` to
`duckfleet/<namespace>/lake`, enforce the dedicated worker node pool/caps, and
set `historyRetentionDays=1` for the synthetic pilot. The upstream default of
zero is unlimited retention and is not the pilot policy.

Do not run the upstream namespace script as an alternative infrastructure
control plane: cloud IAM stays in this repository's Terraform and Kubernetes
resources stay in reviewed Helm/boundary manifests. Its idempotent secret
creation and namespace-local catalog/network model are the implementation to
reuse. Never share catalog passwords or bootstrap tokens between deployments.
Allow only the ER backend's namespace and pod labels to reach the gateway;
keep Postgres inaccessible outside its dataset namespace. Do not expose an
Ingress, public service or customer-facing DuckFleet credentials.

Start with `provisioning`. Install identities, network policies, quota, private
catalog and fresh credentials before publishing `active`. Prove own-data
read/write and foreign catalog/storage/session/history refusal first. Publish
registry and projection as matching generations; a partial rollout refuses
admission until both agree. Retry provisioning against the same immutable IDs
and preserve catalog/PVC/objects; do not rotate passwords during a retry.

## Rotation and revocation

The platform owner first publishes `draining`, refuses new admissions and waits
for existing sessions/attempts to finish or cancels them. Revoke the upstream
issued credential (or rotate the deployment bootstrap credential and restart
all holders), then project a fresh token and increment generation. Verify the
old token is refused by the real gateway before publishing `active` again.
A mismatched projection remains unavailable and can be repaired without data
loss. Never roll the generation backwards to recover a failed rotation.

Emergency revocation publishes `revoked`, revokes upstream credentials and
stops the deployment if sessions cannot be drained. Remove storage grants only
after outstanding writes are fenced. This sequence prevents an old admitted
session from continuing merely because its registry snapshot predates revocation.
No automatic retry may re-admit an attempt carrying an obsolete generation.

## Deprovisioning and retention

Publish `draining`, finish/cancel operations, publish `revoked`, and revoke
credentials before deleting compute or Pod Identity. Keep the tombstone until
all queued attempts and session references have expired. Back up the catalog
and record the data retention/deletion decision before removing its PVC.
Object-prefix deletion is a separate, explicit data-deletion action; retries
must never silently erase artifacts. A failed cleanup leaves revoked state and
retained data, so it can resume safely.

History is private to the dataset's platform database and operator access.
It contains SQL and may contain record values, so customers do not get the
DuckFleet history API. Use bound parameters and avoid values in SQL/context.
The pilot's one-day history expiry does not expire the usage ledger or backups;
those need a separate retention decision before production qualification.

## Evidence and release boundary

The Rust tests cover same-org and cross-org resolution, location overrides,
duplicate ownership, token reuse/substitution, generation mismatch, expiry,
revocation and fixed non-secret errors. Terraform mock tests cover disjoint
object/list grants and the namespace recipe; the existing runner tests prove
the complete Pod Identity trust tuple and restricted KMS use.

Run `PYTHON=python3.12 bash scripts/duckfleet/run-isolation.sh` for three
**disposable** deployments. It tests distinct catalog/artifact sentinels, foreign
credential/read/write refusals, foreign session read/delete and query-history refusals in
all six directions, and absence of tokens from persisted history. This local
probe does not establish live AWS IAM or EKS NetworkPolicy behavior.

The reused upstream implementation is pinned to
[778dbe0](https://github.com/athvin/duck-fleet/tree/778dbe023203570f55e1265a91d4ec26885b250a)
(DuckDB 2.0.1; previously 672b3e3),
including `scripts/provision-namespace.sh`, `infra/helm/duckfleet/templates/network.yaml`,
`identities.yaml`, and `server/src/credentials.rs`. No shared-deployment
multi-tenant authorization claim is made. The HTTP adapter (#1130) and task
integration (#1132) must use the resolver before any remote request. Live
cross-dataset object/catalog/network probes and production enrollment remain
release gates in #1134; a static policy test is not live isolation evidence.
