# Infrastructure design: AWS and Terraform for the ER platform

*The current-state AWS footprint that runs the platform specified in [backend-design.md](backend-design.md): a single AWS account in `us-east-2`, one EKS cluster per environment, a namespace-per-developer dev environment, and a security model where prod is readable by the team but writable only from an explicitly assumed role. Revision 4, 2026-10-11, single account (`us-east-2`). Product overview: [../README.md](../README.md). Revision history, the full rationale, and every rejected alternative live in [infrastructure-decisions.md](infrastructure-decisions.md).*

> This document is the current-state design; live build status is tracked in [plan-to-aws.md](plan-to-aws.md).

---

## 1. Context and goals

The platform today runs as host processes against a Docker Compose substrate: `uvicorn` for the API, `python -m erserver.dispatcher` for the dispatcher, `next dev` for the UI, with Postgres and MinIO in containers ([frontend/dev/compose.yaml](../frontend/dev/compose.yaml), `make run`). That is a correct development stack and a dead end for anything else — there is no deployable environment, no registry, and no notion of dev versus prod anywhere in the repository.

This design adds that substrate, with five goals:

- **Run the four deployables on EKS.** `er-api`, `er-dispatcher`, `er-runner` ([backend-design.md](backend-design.md) §4) plus the Next.js BFF ([frontend-design.md](frontend-design.md) §4.3).
- **Give every developer a complete, isolated environment** in their own namespace, with its own S3 lake prefix, DuckLake catalog databases, and transactional control-plane database — created, suspended, and destroyed in seconds.
- **Hold real production data in lower environments**, protected by IAM, encryption, and audit rather than by scrubbing or synthesis.
- **Make prod readable by the whole team and writable only from an explicitly assumed role**, enforced structurally rather than by policy correctness.
- **Resolve every secret from AWS Secrets Manager** through the `secret://` indirection that already exists in [server/src/erserver/secrets.py](../server/src/erserver/secrets.py).

**Decisions taken** (each developed in its own section; rationale and rejected alternatives in [infrastructure-decisions.md](infrastructure-decisions.md)): a single account in `us-east-2` (§3); IAM Identity Center with permission sets; one EKS cluster per environment, dev-first with prod deferred (§5.4, §15), on **EKS Auto Mode** (§5.5) with `c6id`-family runners; in-namespace Postgres in dev, Aurora in prod (§7.1); idle cost handled by **suspending the namespace** (§5.2); a native Kubernetes Job per run (§6.1) sized vCPU/spill-tiered per record count (§6.5); OpenTelemetry to a swappable collector (§10); cost attributed by tags + CUR + EKS split allocation (§16.1); SOC 2 by building only what cannot be retrofitted (§8.6); **dev behind an internal ALB over Tailscale, prod behind WAF, `er-api` on no load balancer** (§4.1); SES email (§7.5, §9.3); Terraform/Helm applied from CI on `main` (§14.2); and direct-OIDC product SSO in the BFF ([frontend-design.md](frontend-design.md) §2.4).

**Deliberately deferred:** the prod cluster and prod Aurora (no prod traffic yet — §15); the staging environment (no users yet); `arm64`/Graviton images (CI runs on x86, and multi-arch buildx via QEMU hurts the inner loop); multi-region anything, and with it any real DR posture (§18.6).

## 2. Constraints the application imposes

These are not preferences. Each comes from the existing implementation, and the infrastructure has to accommodate it rather than the reverse. They are collected here because most are invisible from a Terraform file and expensive to discover by outage.

| Constraint | Source | Consequence for infrastructure |
|---|---|---|
| `max_locks_per_transaction = 1024` | [docker/compose.yaml:47](../docker/compose.yaml), [frontend/dev/compose.yaml:40](../frontend/dev/compose.yaml) — DuckLake's maintenance sweep drops every expired inlined-data table in one transaction, and the default of 64 fails it with "out of shared memory", poisoning the catalog connection | A cluster parameter group in prod; a `-c` flag on the pod in dev, exactly as both compose files already do |
| `ERSERVER_MAINT_DSN` needs `CREATEDB`; one `CREATE DATABASE` per tenant | `tenant_db_name` at [server/src/erserver/provision.py:79](../server/src/erserver/provision.py) | Rules out Aurora Serverless v1 and DSQL. Serverless **v2** is fine. A dedicated `er_provisioner` role, never superuser |
| Advisory locks are session-scoped and held for a run's lifetime | `tenant_lock` in [src/er/lake/catalog.py](../src/er/lake/catalog.py) | No RDS Proxy or transaction-mode pooler between runner and catalog. A dying pod releasing its lock is designed behaviour, not an accident |
| The job queue and the tenant advisory lock are described as needing one Postgres cluster | [backend-design.md](backend-design.md) §5, [src/er/lake/catalog.py](../src/er/lake/catalog.py) | One Postgres *instance* per environment holding the control-plane DB and every tenant catalog DB. **Worth verifying** — PostgreSQL advisory locks are scoped per database, not per cluster, so this may be over-stated (§18.1). The design satisfies it either way |
| `ERSERVER_CONFIG_ROOT` and `ERSERVER_DROP_ROOT` are written by the API and read by the runner — and the dispatcher — *by path* | [server/src/erserver/api.py](../server/src/erserver/api.py) import endpoint, [configsvc.py](../server/src/erserver/configsvc.py); `drain_staged` re-loads an org's config each idle pass at [server/src/erserver/dispatcher.py:352](../server/src/erserver/dispatcher.py) | A shared writable filesystem — EFS with the CSI driver — mounted by the API, the dispatcher, *and every runner Job* (§6.2). Pod-local storage cannot work, and this is what blocks a stateless multi-replica API today |
| DuckDB's temp directory is CWD-relative (`/app/.tmp`, `/app/dbt/.tmp`) and no environment variable relocates it | [benchmarks/storage_sampler.py:29](../benchmarks/storage_sampler.py) | Ephemeral volumes mounted at exactly those two paths (§5.3) |
| 141.48 GiB of *live* spill sampled on the 10M-record full reload, at a **4 GB** DuckDB memory limit in a 10 GiB container | [performance.md](performance.md) | NVMe instance store, not EBS. Ten million records fitting in 10 GiB is also why sizing is CPU-tiered rather than memory-tiered (§6.5) |
| 2 → 6 DuckDB threads cut a comparable 10M reload **40%** with peak memory flat (9.14 vs 9.37 GiB) — a work-in-progress single-pass measurement (§6.5) | [performance.md](performance.md) | vCPU is the time lever and memory is nearly constant in corpus size. Compute-optimized `c6id` nodes, not memory-optimized `r6id` (§5, §6.5) |
| The dispatcher's startup reaper assumes it is alone | `queue.reap_stale(leader)` at [server/src/erserver/dispatcher.py:379](../server/src/erserver/dispatcher.py) | `replicas: 1` with `strategy: Recreate` — never RollingUpdate |
| One active job per org, enforced by the `jobs_one_active_per_org` partial unique index | [server/src/erserver/db.py](../server/src/erserver/db.py) | A tenant scales *up*, never *out*. Size one runner to the largest tenant; scale `ERSERVER_CONCURRENCY` only for cross-tenant throughput |
| Exit code `None` — killed, OOM, evicted — requeues with `--resume`, bounded by `max_attempts` (default 3) with exponential backoff | `dispose()` in [server/src/erserver/policy.py](../server/src/erserver/policy.py) | Spot interruption is survivable by design, which is what makes spot safe for runners. It is also why eviction is *expensive but bounded* rather than fatal — see §5.3 |
| The dispatcher holds a permanent leader connection and polls every `ERSERVER_POLL_SECONDS` (default 2s) | [server/src/erserver/dispatcher.py](../server/src/erserver/dispatcher.py), [settings.py](../server/src/erserver/settings.py) | **No database that bills by connection-idleness will ever idle.** This killed Aurora auto-pause as a dev cost strategy (§7.1) |
| DuckDB reads neither cgroup limits nor host core count | [src/er/lake/ducklake.py](../src/er/lake/ducklake.py) | Every pod limit must be paired with an explicit `ER_DUCKDB_THREADS` / `ER_DUCKDB_MEMORY_LIMIT`. A pod limit alone does nothing |
| API read-path attaches inherit the *tenant's* DuckDB env | [server/src/erserver/readapi.py](../server/src/erserver/readapi.py) via `resolve_env`, `_POOL_CAPACITY = 8` warm attaches per process | Setting a tenant to 40 GB / 12 threads also sizes the API pods' DuckDB. [backend-design.md](backend-design.md) §8 says API pods *should* pin ~2 GiB / 2 threads; that split is not implemented, so API pod limits must be set defensively |
| The boot order API-then-dispatcher is load-bearing | [frontend/dev/up.sh](../frontend/dev/up.sh) — the dispatcher's leader connection blocks the API's `ensure_schema` DDL indefinitely when they start together | An init container on the dispatcher gating on the API's `/healthz` |

## 3. Account and organization topology

**One account.** Everything lives in a single AWS account, `er` (797781631727): Identity Center, billing, the Object-Locked audit archive (§8.6), ECR, Terraform state, dev environments, and — behind the §15 prod gate — prod, separated by prefix and role, not by account.

```
Organization root (kept — Identity Center requires it; otherwise inert)
└── er (797781631727)       everything, separated by prefix and permission set
```

Environment separation is by **name and by role**: resources carry an `er-dev-*` / `er-prod-*` prefix (buckets, roles, clusters, secrets), and the Identity Center permission sets scope who may touch which prefix (§8.1, §8.2).

**What the single account trades away** — recorded deliberately, accepted by the operator (full argument: [infrastructure-decisions.md](infrastructure-decisions.md) D-ACCT):

- **SCPs no longer apply to anything** — every §8.2 guardrail is an IAM-layer deny carried by the permission sets, enforceable but one policy edit from absent.
- **Blast radius is shared** — dev mutation is unrestricted by design (§8.1) and now shares an account, and its service quotas, with prod data.
- **The audit archive lives with the workloads** — the compensating control is S3 Object Lock in compliance mode, 400-day retention not even root can shorten (§8.6).
- **SOC 2 CC6 is evidenced from policy text** rather than account membership.

The former `nonprod` member account (660360495170) is parked empty at the organization root at zero cost; close it or re-adopt it if the split returns.

## 4. Network, per environment

One VPC per environment, `/16`, three availability zones, public and private subnets.

- **An S3 Gateway Endpoint is mandatory, not an optimization.** A large run moves hundreds of gigabytes between the runner and the lake; routing that through a NAT Gateway at $0.045/GB would dominate the bill. The gateway endpoint is free and keeps the traffic on the AWS backbone.
- **One NAT Gateway in dev**, three in prod for AZ independence once prod carries traffic. The single dev NAT means cross-AZ traffic from the other two subnets at $0.01/GB each way — real but small next to a per-AZ NAT.

**VPC interface endpoints are deliberately not used** — non-S3 traffic goes through the NAT. The reasoning (they cost ~$110/mo per environment against the $33 NAT, and ECR layer blobs ride the free gateway endpoint anyway) is in [infrastructure-decisions.md](infrastructure-decisions.md) D-ENDPOINTS. Revisit when per-GB NAT processing exceeds the fixed endpoint cost, or a compliance requirement forbids the NAT path.

### 4.1 Edge access: who can reach these environments at all

§13 deliberately fills developer namespaces with real production records, so the edge deserves the same reasoning §8.2 spends an account boundary on — an internet-facing ALB would leave the Next.js session check as the only thing between those records and the internet.

**Dev: an internal ALB, reached over Tailscale.** A subnet router on a `t4g.nano` advertises the VPC CIDR and resolves the private zone via the VPC resolver — roughly $4/mo, and Tailscale's free tier covers three users. (Managed VPN alternatives and ALB OIDC were considered and rejected — D-EDGE.)

**Prod, when it exists: internet-facing, behind WAF.** Customers arrive from the internet, so the boundary moves from network to WAF plus application auth. The non-negotiable rules are rate-based: the frontend exposes five unauthenticated paths — six once SSO lands, with [frontend-design.md](frontend-design.md) §2.4 adding `/api/auth/sso` — and three of them — `/api/auth/login`, `/api/auth/password-reset` (which also sends email), and `/api/invites` — accept unauthenticated writes with no in-app rate limiting anywhere today; the SSO start and callback routes join the rate-ruled set the day they exist. WAF rate rules scope to URI paths with thresholds down to 10 requests per window, at roughly $8/mo all-in.

**`er-api` is on no load balancer in any environment.** Already true by construction, and it needs only to be kept true: the frontend defines zero `NEXT_PUBLIC_*` variables by stated policy, every browser call is same-origin to the BFF, and `ERSERVER_BASE_URL` lives in server-only modules. Only `er-web` gets an Ingress; erserver stays a ClusterIP service. One loose end to close while here: FastAPI serves `/docs`, `/redoc`, and `/openapi.json` unauthenticated by default and nothing disables them (§14.1) — harmless in-cluster, pointless to leave on.

## 5. Compute: EKS

EKS per environment, on the newest minor current at build time (1.36 as of this revision), pinned in Terraform rather than here. API-based access entries rather than the legacy `aws-auth` ConfigMap, and these addons: EBS CSI, EFS CSI, AWS Load Balancer Controller, external-dns, External Secrets Operator. Node management is **EKS Auto Mode** (§5.5), so there is no self-managed Karpenter or system node group to operate.

| NodePool | Instance types | Capacity type | Serves |
|---|---|---|---|
| `app` | `m6i.large`, `m6a.large` | spot in dev, on-demand in prod | `er-api`, `er-web`, `er-dispatcher` |
| `runner-sm` | `c6id.2xlarge`, `c8id.2xlarge`, `c5ad.2xlarge`, `c5d.2xlarge`, `m6id.2xlarge` — 8 vCPU, local NVMe | spot, on-demand fallback | S and M classes |
| `runner-l` | `c6id.4xlarge`, `c8id.4xlarge`, `c5ad.4xlarge`, `m6id.4xlarge` — 16 vCPU, local NVMe | spot, on-demand fallback | L class |

The runner pools are compute-optimized with local NVMe (`c6id` and siblings), not memory-optimized `r6id`, and list many families plus both capacity types deliberately (rationale: D-NODE, D-SPOT). One exclusion is load-bearing: **`c5d.4xlarge` carries only 400 GB of NVMe against the L class's 400 GiB spill limit (§5.3)** — negative headroom before image and kubelet overhead — so it stays out of `runner-l`, while `c5d.2xlarge` (200 GB against S/M's 40–150 GiB) stays in `runner-sm`. `c6id.8xlarge` (32 vCPU) is the top of the ladder, bounded by the `le=32` cap on `duckdb_threads`. Spare capacity is not waste: the one-active-job-per-org constraint means two *different* tenants' runs bin-pack onto one node, and at ~14 GiB per L pod a `c6id.4xlarge` holds two.

**Account vCPU quota is the real floor on node size, and it bites a fresh account.** A new AWS account defaults the two quotas these NodePools draw on — *All Standard Spot Instance Requests* (`L-34B43A08`) and *Running On-Demand Standard instances* (`L-1216C47A`) — to **8 vCPU each**, measured in vCPUs, not instances. A single `runner-l` node is 16 vCPU, so under the default a class-L run can never place: Karpenter's fleet request fails `VcpuLimitExceeded`/`MaxSpotInstanceCountExceeded`, the NodeClaim is torn down and retried, and the runner pod sits `Pending` forever while the dispatcher dutifully keeps the job active (observed 2026-10-11, first 10M run). Both quotas are raised to **64 vCPU** (us-east-2) — enough for the 2-vCPU app node plus concurrent L and M runs with headroom, short of the ~128+ range that draws manual review. This is the §8 "dev shares prod's service quotas" hazard made concrete; revisit before Phase 3 onboards a real tenant.

### 5.1 Karpenter disruption settings

The runner pools must not use the default/cost-reaching `WhenEmptyOrUnderutilized` — on a busy node that discards up to an hour of work, and the retry policy in §2 makes it register as unexplained slowness rather than an incident (D-DISRUPT).

| NodePool | `consolidationPolicy` | `consolidateAfter` | Pod annotation |
|---|---|---|---|
| `app` | `WhenEmptyOrUnderutilized` | `1m` | — |
| `runner-sm`, `runner-l` | **`WhenEmpty`** | `30s` | **`karpenter.sh/do-not-disrupt: "true"`** |

`karpenter.sh/do-not-disrupt` on the runner pods blocks voluntary disruption for the life of the job; nodes still empty ~30s after the pod exits; `expireAfter` (72h) handles AMI hygiene, since a do-not-disrupt pod otherwise pins a node indefinitely. Spot interruption is *involuntary* and cannot be annotated away — Karpenter drains on the two-minute notice and `--resume` picks up at the last completed stage.

### 5.2 Idle cost is a namespace-suspend problem, not an autoscaling one

A developer namespace running `er-api`, `er-dispatcher`, and `er-web` around the clock keeps an `app` node alive permanently, and no Karpenter setting changes that (why: D-IDLE). The control that works is suspending the namespace:

```
make dev-suspend DEV=alice     # scale all Deployments + the Postgres StatefulSet to 0
make dev-resume  DEV=alice     # scale them back; PVC and S3 prefix untouched
```

This cascades: the `app` node empties and is reclaimed, the dispatcher's permanent leader connection goes away, and the Postgres pod stops billing compute while its EBS volume persists. A `CronJob` suspends idle namespaces. There is no request log in the system, so the predicate is built on `erweb.sessions.last_seen_at`, slid on activity but throttled to at most hourly ([frontend/src/lib/auth/session.ts:13](../frontend/src/lib/auth/session.ts)): **suspend when `max(last_seen_at)` is older than four hours *and* no job is queued or running** — the second clause keeps the CronJob from burning a `--resume` attempt under a live run. Resume is a single `helm upgrade`, under a minute.

### 5.3 Spill, ephemeral storage, and the eviction trap

DuckDB writes temp files to a `.tmp` directory relative to the process working directory, and nothing in the engine emits `SET temp_directory`. The two concrete paths are `/app/.tmp` for the CLI and runner and `/app/dbt/.tmp` for the dbt subprocess ([benchmarks/storage_sampler.py:29](../benchmarks/storage_sampler.py)).

Both runner NodePools set `instanceStorePolicy: RAID0` on their EC2NodeClass, striping the local NVMe and making it the kubelet's ephemeral-storage root; an `emptyDir` then lands on NVMe automatically, so mounting `emptyDir` at those two paths satisfies the spill requirement with no engine change.

The runner pods carry an explicit `requests.ephemeral-storage` (per class) — which is also what lets the scheduler avoid stacking two spill-heavy pods onto one node — and an `emptyDir.sizeLimit` generous enough for the real figure plus headroom but below the node's capacity. On a 950 GB `c6id.4xlarge` that is roughly 400 GiB per L runner, which fits two. Both limits are load-bearing: with none, one runaway run evicts every pod on the node under disk pressure; set too low, the runner itself is evicted partway through, requeues with `--resume`, and evicts again at the same point until `max_attempts` is spent (the eviction trap — D-SPILL).

Crucially, the 141 GiB figure is measured at a **4 GB** DuckDB memory limit, and §6.5 keeps the L class at 8 GB — so the measurement was taken near the configuration this design actually runs, and transfers. Spilling heavily to free instance-store NVMe instead of buying RAM to avoid it is the deliberate trade (§6.5).

### 5.4 One cluster per environment

One EKS cluster per environment, for cluster-scoped blast-radius containment, a staging-shaped upgrade path, and — the decisive argument — secret separation. Two clusters are kept in the design but **only dev is built now**: there is no prod traffic, so deferring the prod cluster costs nothing and preserves the boundary (§15 sequences it). The rejected single-cluster-total option and the full secret-separation argument are in [infrastructure-decisions.md](infrastructure-decisions.md) D-CLUSTER.

### 5.5 EKS Auto Mode — chosen (D1, 2026-10-10)

Auto Mode manages Karpenter and the core addons, removing the system node group and the burden of upgrading them. It honours `karpenter.sh/do-not-disrupt` across every consolidation policy and exposes `disruption.consolidationPolicy: WhenEmpty` with `consolidateAfter`, so the §5.1 runner controls survive verbatim; it formats instance-store NVMe RAID0 automatically (§5.3), enforces IMDSv2 hop-limit-1 unconditionally (§5.6), and ships SOCI parallel pull on by default (§11.1). Cost is ~12% of the on-demand rate per instance-hour (charged even on Spot) — roughly $9/mo for the always-on app node and cents per runner-hour. **It costs two things this design otherwise wanted:** no custom AMIs or `blockDeviceMappings`, so §11.1's snapshot-prebake rung is off the table; and since April 2026 its managed instances are hidden from `DescribeInstances`. **Revisit trigger:** if the Auto Mode fee line in the CUR exceeds ~$50/mo, re-run against self-managed Karpenter. Full history (declined rev 2, reopened rev 3, chosen D1): [infrastructure-decisions.md](infrastructure-decisions.md) D-AUTOMODE.

### 5.6 Node and pod hardening

- **IMDSv2 required, hop limit 1, on every node.** This stops a pod — including a runner processing tenant data — from harvesting the node role off the metadata service. It breaks nothing this design runs: kubelet, the VPC CNI, and the CSI drivers are hostNetwork and keep their access; IRSA never touches IMDS; Phase 1's static S3 keys arrive by env var. Auto Mode enforces it without asking (§5.5).
- **Pod Security Standards labels on every namespace** — `baseline` enforced from day one, `restricted` the target. The gap to `restricted` is that no image declares a `USER` today (the engine runs as root), which is an application change (§14.1).
- **Cluster API-data encryption: the default is enough.** EKS envelope-encrypts all Kubernetes API data with an AWS-owned key on every 1.28+ cluster. A customer-managed key is deliberately *not* associated — association is irreversible and a disabled or deleted CMK degrades the cluster unrecoverably, risk with no cross-account payoff (same reasoning as the ECR exception in §8.3; detail in D-KMS).

Account-level counterparts — S3 Block Public Access on the account, MFA on the root user, no root access keys — are one-liners that live in Phase 0 (§15).

## 6. Job execution: the runner as a Kubernetes Job

A pipeline run is one ephemeral pod, scoped to one tenant, that exits when the run finishes. That is what [backend-design.md](backend-design.md) §4 already specifies, and `server/README.md` records the k8s Job launcher as deliberately unbuilt behind an existing seam:

```python
dispatcher.launch_runner(payload, env, on_stage=, should_cancel=)
```

Today that seam calls `subprocess.Popen([sys.executable, "-m", "erserver.runner", ...])` with the tenant's `ER_*` env merged in. Substituting a Kubernetes Job is a change to one function, which is the whole reason the seam exists. Pod-per-run (rather than long-lived workers) is forced by three facts: the tenant seam is process-scoped env ([src/er/lake/env.py](../src/er/lake/env.py) is the sole reader of `ER_*`); the Postgres advisory writer lock is session-scoped, so a dying pod releases it with no cleanup path; and memory bin-packing against an 8–64 GiB per-run envelope only works if the allocation is per-run.

### 6.1 Argo Workflows — not chosen

A native Kubernetes Job per run, not Argo: the DAG is inside the process (`run_all_full` runs the stages as library calls in one pod), and the engine already owns a complete, tested retry/state/scheduler machine that Argo would duplicate and contradict — including the `jobs_one_active_per_org` guarantee no second scheduler can honour. Full argument and revisit triggers: [infrastructure-decisions.md](infrastructure-decisions.md) D-ARGO. (Argo **CD** for deploying the platform itself is a separate, out-of-scope question.)

### 6.2 The Job spec, and what it has to get right

```
Job (one per run, in the platform namespace)
  backoffLimit: 0                      ← the engine owns retries, not Kubernetes
  restartPolicy: Never
  activeDeadlineSeconds: per class     ← S 2h / M 6h / L 24h
  ttlSecondsAfterFinished: 3600        ← long enough for the dispatcher to read final status
  annotations:
    karpenter.sh/do-not-disrupt: true  ← §5.1
  nodeSelector: er/pool=runner-{sm,l}
  resources:
    requests.memory: per class (8/24/64Gi)
    requests.ephemeral-storage: per class   ← §5.3
  volumes:
    emptyDir → /app/.tmp, /app/dbt/.tmp     ← §5.3
    EFS access point → ERSERVER_CONFIG_ROOT, ERSERVER_DROP_ROOT   ← §7.4, the Job cannot run without it
  envFrom: per-run Secret (the tenant's ER_* overlay)
```

**`backoffLimit: 0` is the single most important field.** The default of 6 would have Kubernetes retry a failed run with no knowledge of the exit-code taxonomy — re-running a config error six times, and worse, re-running a partially-completed pipeline *without* `--resume`. **The EFS mount is the second most important line, because its absence fails late:** the runner's first act is opening the config file by path ([server/src/erserver/runner.py:31](../server/src/erserver/runner.py)), an incremental import lists its CSV drops from `drop_root` at ingest time ([src/er/ingest/sources.py:146](../src/er/ingest/sources.py)), and the dispatcher re-loads an org's config on every idle pass (§2). Config content could in principle travel inline, but drops cannot leave the filesystem (`storage.drop_dir` must be an absolute local path, [src/er/config/schema.py:285](../src/er/config/schema.py)); inline config is a later optimization, not a way out of the mount (D-JOBSPEC).

Cancellation keeps today's semantics: the dispatcher deletes the Job, Kubernetes sends `SIGTERM`, and the engine stops at the next stage boundary. `terminationGracePeriodSeconds` must exceed the engine's stage-boundary check interval and the dispatcher's existing `TERMINATE_GRACE_SECONDS = 10.0`.

The dispatcher's ServiceAccount gets a namespace-scoped **Role**, not a ClusterRole: `create`/`get`/`list`/`watch`/`delete` on `batch/jobs`, `get`/`list` on `pods`, and `get` on `pods/log` — the last for exactly one tail fetch *after* a Job reaches a terminal state, to recover the runner's terminal result line (the S4.7 `error_class` the §2 retry matrix branches on). That read is not the §6.3 progress stream and must never become one. Namespace-scoped matters — a dispatcher able to create pods cluster-wide would undercut §5.4's argument for separate clusters.

### 6.3 Progress reporting

Today `launch_runner` reads the subprocess's stderr pipe directly, parsing one JSON record per stage into `jobs.progress`. A Job pod has no such pipe, so **the runner writes its own progress** to the control-plane database — it already holds a DSN and already emits exactly these records, so the write becomes direct instead of relayed. This survives dispatcher restarts, removes N long-lived log streams, and sits on the same seam §10 needs for trace context, so the two are designed together. (The rejected dispatcher-log-streaming option is in D-PROGRESS.) It is a small change to [server/src/erserver/runner.py](../server/src/erserver/runner.py).

### 6.4 "Contained to that tenant" — what the containment actually is

[backend-design.md](backend-design.md) §7 makes a tenant a **database, a schema, and an S3 prefix** — not a namespace. In prod every tenant's runs execute as Jobs in one platform namespace; the per-namespace split in §12 is per *developer*, not per tenant. A run's isolation comes from four things, none of them a namespace boundary:

- **Process-scoped env.** `ER_CATALOG_DSN`, `ER_LAKE_DATA_PATH`, `ER_LAKE_METADATA_SCHEMA` injected per run; the pod can address exactly one tenant's catalog and prefix for its whole life.
- **Ephemerality.** The pod exits with the run, so no long-lived process ever holds two tenants' credentials.
- **A dedicated catalog database** per tenant, so no tenant's tables share a database with another's.
- **The advisory writer lock**, which the pod holds for its session and releases by dying.

Namespace-per-tenant was rejected (D-TENANCY) — it is the right shape for the enterprise tier [backend-design.md](backend-design.md) §7 earmarks for dedicated buckets and STS-scoped credentials, and not before. Forward-looking: once §9.2's IRSA work lands, the per-run pod can carry a **per-tenant ServiceAccount** scoped to that tenant's prefix alone (the "per-tenant STS credentials" item in [backend-design.md](backend-design.md) §15 phase 4), so the Job spec should anticipate a ServiceAccount parameter even while every run shares one.

### 6.5 Sizing the Job: CPU is the lever, memory is nearly flat

Because the dispatcher knows the tenant's record count before it launches anything, the Job is sized per run rather than drawn from a fixed class. The resource classes are **vCPU/spill-tiered with a near-constant memory floor** (per [server/src/erserver/sizing.py](../server/src/erserver/sizing.py)), and [backend-design.md](backend-design.md) §7 has been corrected to match. The measurements behind the inversion from the original memory-tiered classes, and why memory stays flat (DuckDB spills), are in [infrastructure-decisions.md](infrastructure-decisions.md) D-SIZING.

**Class shape — tiered on vCPU, with a near-constant memory floor:**

| Class | Records | vCPU / `ER_DUCKDB_THREADS` | Pod memory | `ER_DUCKDB_MEMORY_LIMIT` | Ephemeral storage |
|---|---|---|---|---|---|
| S | ≤100k | 2 | 6 GiB | 4GB | 40 GiB |
| M | ≤1M | 6 | 10 GiB | 6GB | 150 GiB |
| L | ≤10M | 12–16 | 14 GiB | 8GB | 400 GiB |

Pod memory grows a little because peak container usage was 9.37 GiB against a 4 GB DuckDB limit (Python heap, Arrow buffers, the dbt subprocess); ephemeral storage is sized from measured spill. Two invariants are load-bearing and tested rather than trusted:

- **`ER_DUCKDB_THREADS` and the CPU request are set in the same operation** — §2 records that DuckDB reads neither cgroup limits nor host core count, so `threads` out of step with the vCPU request wastes cores or thrashes, neither failing loudly.
- **`ER_DUCKDB_MEMORY_LIMIT` stays strictly below the pod memory limit** — the invariant [benchmarks/scales.py](../benchmarks/scales.py) validates (`duckdb_memory_limit < mem_limit`); exceeding it means the OOM killer rather than a graceful spill.

**The plumbing is half-wired.** `PATCH /v1/orgs/{org}/resources` ([server/src/erserver/api.py:451](../server/src/erserver/api.py)) accepts `duckdb_threads` (bounded `ge=1, le=32`) and `duckdb_memory_limit`, merges them into `orgs.env`, resolved at **claim** time. What is missing: deriving the values from record count ([benchmarks/scales.yaml](../benchmarks/scales.yaml) maps counts to envelopes, though its generous memory figures should not transfer) and translating them into pod `resources`. The `le=32` bound caps the useful instance at 32 vCPU — `c6id.8xlarge` is the top of the ladder.

**Two honest limits.** The thread-scaling measurement stops at 6 threads, so the knee above 6 is unknown — and unlikely to be linear, because the same run showed **reconcile 14% *slower*** at 6 threads, a Python graph step that does not parallelize with DuckDB threads (§18.2). And dynamic sizing makes the §2 API-pod inheritance problem worse: per-run sizing is therefore carried on the **job row**, not merged into `orgs.env`, until the [backend-design.md](backend-design.md) §8 split between tenant and API-pod settings is implemented (§18.10).

## 7. Data plane, per environment

### 7.1 Postgres: a pod in dev, Aurora in prod

The DuckLake catalog is the least ephemeral thing in the system (table definitions, the snapshot log, the table-version→parquet map; lose it and the S3 objects are an unreadable pile), but it does not need a managed database server standing up for one developer (why not Aurora auto-pause in dev: D-AUTOPAUSE).

| | dev (per namespace) | prod |
|---|---|---|
| Form | `postgres:16` StatefulSet, one per developer namespace | Aurora PostgreSQL 16 Serverless v2 |
| Storage | 50 GiB gp3 PVC | Aurora storage |
| Capacity | 1 vCPU / 2 GiB | 0.5 → 16 ACU |
| `max_locks_per_transaction` | `-c` flag on the pod | cluster parameter group |
| Availability | single-AZ, pinned by its EBS volume | multi-AZ |
| Backups | none — prod is the system of record | 35-day PITR |
| Idle cost | **~$4/mo of EBS** | ~$50/mo |

Each instance — pod or cluster — holds the control-plane database *and* that environment's tenant catalog databases:

```
dev-alice/postgres-0              prod/aurora-prod
├── erctl         control plane   ├── erctl
└── t_acme_<hash> tenant catalog  └── t_acme_<hash>
```

This satisfies the §2 shared-instance constraint by construction (queue and advisory lock in one instance), so the design holds whether or not that constraint is real (§18.1). A pod is also closer to tested parity — both compose files run `postgres:16` with `-c max_locks_per_transaction=1024` — and its EBS volume is AZ-bound, keeping the runner→catalog connection in-AZ.

Roles, both forms: `er_app` for application traffic, `er_provisioner` with `CREATEDB` used only by `ERSERVER_MAINT_DSN`, and in prod `er_readonly` authenticated by IAM (`rds_iam`, no password exists) for the §8.4 read path.

**DDL ledger install:** the `er_ddl_audit_*` event triggers (the `ddl_log` ledger) need superuser/rds_superuser — neither `er_app` nor `er_provisioner` qualifies. Boot applies them best-effort (succeeds in CI/dev where the role is superuser; logs a warning otherwise); in production they are installed once by running `er-server migrate` with the Aurora master-user DSN as part of the deploy.

**Every DSN carries `sslmode=verify-full` in prod** (`sslmode` appears nowhere in the repo today, an encryption-in-transit gap against Aurora an auditor will ask about — §8.6). It affects `ERSERVER_DSN`, `ERSERVER_MAINT_DSN`, `ERSERVER_TENANT_DSN_TEMPLATE`, `ERWEB_DATABASE_URL`, and `ER_CATALOG_DSN` — a secret-value change, not code, provided `verify-full` works with the Aurora CA bundle in the images (§18.15). Dev keeps `sslmode=disable` against the in-namespace pod.

### 7.2 The tenant database name collision

`tenant_db_name(org)` at [server/src/erserver/provision.py:79](../server/src/erserver/provision.py) returns `er_` + `t_<org>_<blake2b(org)[:8]>`. The hash covers the org name alone, so two developers who each provision an org called `acme` derive the same database name.

With Postgres per namespace (§7.1) this is no longer a collision — the databases are in different instances. The fix is still worth shipping, to keep a shared staging/dev cluster viable: a new optional `ERSERVER_TENANT_DB_PREFIX`, defaulting to `er_` so current behaviour is bit-for-bit unchanged. It prefixes the **database name only**, leaving `tenant_namespace()` — and therefore the metadata schema and S3 prefix component — untouched, so a tenant's lake layout stays identical across environments (which is what makes the §13 refresh path tractable).

### 7.3 S3 lake

One bucket per environment: `er-dev-lake`, `er-prod-lake`. Both with SSE-KMS under the environment's own CMK and a bucket policy denying non-TLS requests.

```
s3://er-dev-lake/dev/alice/{ns}/     ERSERVER_LAKE_DATA_PATH_TEMPLATE, per dev namespace
s3://er-prod-lake/{ns}/              prod
```

`ERSERVER_LAKE_DATA_PATH_TEMPLATE` is validated to contain `{ns}` and end with `/` ([server/src/erserver/settings.py:55](../server/src/erserver/settings.py)); both forms satisfy it. Local stacks carry the same shape under the single MinIO bucket: the settings default and the dev stack both use `s3://lake/{ns}/`, and the CI compose stack is its fixed single-tenant instance `s3://lake/er/`. Models stay under the tenant's prefix (`provision.py` sets `model_uri_prefix = f"{plan.data_path}models/"`) so a tenant purge is one prefix delete.

**Versioning carries a lifecycle policy** (why — D-VERSIONING): noncurrent versions expire after 30 days, incomplete multipart uploads after 7. Without it, a lake bucket where every run rewrites parquet grows without bound, and a run killed mid-write leaves parts that bill silently.

**CloudTrail data events are scoped, not blanket.** §8.5 relies on them to attribute reads of real records, but data events bill ~$0.10 per 100,000 and a DuckLake run issues an enormous number of object operations — a single 10M-row run could plausibly generate millions of events. The trail is therefore restricted to read events on the prod lake prefix only, excluding the dev bucket and excluding writes. §18.4 carries the measurement that would justify widening or narrowing it.

### 7.4 EFS

One filesystem per environment with an access point per namespace, mounted at `ERSERVER_CONFIG_ROOT` and `ERSERVER_DROP_ROOT` — by the API, the dispatcher, and every runner Job (§6.2). Elastic throughput. Content is org YAML and CSV import drops — a few gigabytes, ~$1/mo — and no object store substitutes without an application change. EFS automatic backups (AWS Backup default plan, daily, 35-day retention, pennies) is enabled; what it uniquely protects is CSV drops awaiting ingest, since the org YAMLs are a projection of `config_versions.yaml` in the control-plane DB that `provision.seed_config` rewrites on replay ([server/src/erserver/provision.py:218](../server/src/erserver/provision.py)).

### 7.5 Email: SES

Email is live in the Rust control plane as [`er_server::mail`](../rust/crates/er-server/src/mail.rs) behind `POST /v1/email`, which is the BFF's only mail path ([`relayEmail`](../frontend/src/lib/bff/relay.ts)) and therefore the platform's one transport and one template set. Three templates: `invite`, `password_reset`, `signup_verify`. Bodies are `format!`, not a template engine — each required parameter is declared in exactly one place and a missing one is a 422 the caller sees, rather than a blank in a message someone already received.

The Python-era design this replaces had a durable `email_outbox` table drained by the dispatcher's leader tick. That is **not** what shipped: `POST /v1/email` sends synchronously and the caller treats failure as data. Every caller already has a usable fallback — an invite link to copy, an enumeration-safe 202 — so a durable outbox would add a table, a drain loop and a retry policy to make reliable something no flow actually blocks on. If a flow ever does, the outbox is the right answer then.

`ER_MAIL_BACKEND` selects the transport: `ses` sends, `log` renders to the pod log (how a dev namespace gets its links without AWS), and unset answers **503** on every send, which the relay turns into `false`. A misconfigured backend degrades to 503 with a warning rather than failing the boot — mail is best-effort and the control plane has other jobs.

**Transport is the SES API, not SES's SMTP interface.** That inverts the old §9.3 decision, and deliberately: the SMTP endpoint cannot accept credentials derived from temporary ones, which was the *entire* reason §9.3 budgeted a second long-lived IAM access key and then scheduled its retirement for Phase 4. Since the Rust mailer was written fresh rather than ported, signing `SendEmail` with the pod's EKS Pod Identity credentials takes that exit before the credential ever existed. The SDK is configured `default-features = false` to keep two things out of the build: aws-lc-rs, whose `-sys` crate needs cmake the build image lacks, and the SSO credential providers, which require Rust 1.88 and would quietly raise the workspace's declared 1.85.

The [`ses/` module](../infra/terraform/modules/ses/main.tf) provides the domain identity with Easy DKIM (three CNAMEs), a custom MAIL FROM (MX + SPF) with `behavior_on_mx_failure = REJECT_MESSAGE`, and a configuration set with TLS required on delivery, reputation metrics, and account-level suppression for bounces and complaints. It emits a `ses:SendEmail` policy document scoped to the identity and to a From address at the domain; [`envs/dev/ses.tf`](../infra/terraform/envs/dev/ses.tf) attaches it to a role and binds it by Pod Identity association. No IAM user, no Secrets Manager entry.

DNS is not managed by the *module* — the zone is Cloudflare's (§18.8) and the module's provider is AWS — so it emits the records as outputs and the calling environment writes them with the Terraform `cloudflare` provider, which is what §18.8 specified. `envs/dev/ses.tf` holds those three `cloudflare_dns_record` resources, so an apply leaves nothing to paste by hand; the module stays single-provider, which is what lets a caller whose zone lives elsewhere use it unchanged. Two facts land this in Phase 1: production access (sandbox exit) is a human-reviewed request with a ~24-hour first response, so file it early; and the sandbox limits (200 messages/day, verified recipients only) are *fine for dev indefinitely* if the developers' own addresses are verified.

One wrinkle worth knowing: a Pod Identity association binds one (cluster, namespace, service account) triple and there is no wildcard, while dev namespaces are per-developer and created by `scripts/dev_env.sh`/Helm rather than Terraform. `mail_pod_namespaces` is therefore an explicitly-listed, empty-by-default variable: a developer who wants real delivery adds their namespace, and everyone else uses `log`.

## 8. Security model

### 8.1 Identity Center permission sets

Every permission set is assigned in the single account; the "scope" is the resource-name prefix each grants against, not an account.

| Permission set | Scope | Assigned to | Grants | Session |
|---|---|---|---|---|
| `PlatformAdmin` | account-wide minus the §8.2 prod denies | you + developers | Full access; mutating `er-dev-*` is unrestricted by design | 8h |
| `PlatformReadOnly` | account-wide read | developers | AWS-managed `ReadOnlyAccess`, for the agent introspection path (§8.2); reached by the `er-readonly` profile | 8h |
| `ProdDataReadOnly` | `er-prod-*` read | you + developers | `s3:GetObject`/`ListBucket` on the prod lake, `kms:Decrypt` scoped `kms:ViaService = s3`, `rds-db:connect` as `er_readonly`, `eks:DescribeCluster` plus a `view`-only Kubernetes group | 4h |
| `ProdAdmin` | `er-prod-*` full | **you only** | Full prod (the only set without the §8.2 denies) | 1h |

`PlatformReadOnly` exists because `ProdDataReadOnly` cannot serve as a general read identity: its grants are the prod lake, one database role, `eks:DescribeCluster` and logs, so it cannot list buckets or read Budgets. The MCP proxy runs as it, which means the proxy's own `--read-only` flag is not the only thing preventing an agent-initiated write — the credential cannot write either.

Access is granted by **group membership**, not per-person assignment, so onboarding is one entry and offboarding is one removal.

No IAM users exist for humans, and no long-lived access key exists anywhere except **one** service identity — S3 (§9.2) — carrying a scheduled Phase 4 retirement. This said *two* until the mailer landed: SES was going to need its own key because its SMTP interface rejects temporary credentials, but the Rust mailer calls the SES API instead and signs with Pod Identity, so that key was never created (§7.5, §9.3).

Two cautions on `ProdDataReadOnly`. The Kubernetes `view` role must never become `edit` — the built-in `view` excludes Secrets, which is the only thing keeping §9's projected prod secrets out of reach. And read access to prod **pod logs** is a PII path in a system whose whole purpose is handling personal records; the permission set grants `logs:` read for debugging, which is only safe because §10.4 makes it a rule that log lines carry identifiers and counts and never attribute values. That rule is what this grant depends on, and §17 verifies it rather than trusting it.

### 8.2 How prod writes are blocked in one account

With a single account there is no OU and SCPs evaluate against nothing (§3), so the two guardrail statements ride the permission sets as inline denies. `PlatformAdmin` carries AdministratorAccess **plus two deny statements** scoped to the prod prefix; `ProdAdmin` (1h sessions, prod-admins group only) is the only permission set without them:

**Statement 1 — data mutation.** Denies `s3:Put*`/`Delete*` on `er-prod-*` buckets, `rds:Delete*`/`Modify*`, `eks:Delete*`/`Update*`, `secretsmanager:Put*`/`Delete*` and `kms:ScheduleKeyDeletion` on `er-prod-*` / `er/prod/*`-named resources.

**Statement 2 — privilege escalation.** Denies `iam:CreateRole`, `PutRolePolicy`, `AttachRolePolicy`, `UpdateAssumeRolePolicy` and `CreateAccessKey` against `er-prod-*` roles. Statement 1 leaves the application's own `er-prod-*` roles writable paths to the lake, so statement 2 is what keeps a developer from minting `er-prod-app-anything` and walking out of the guardrail — the exemption is only trustworthy because minting the thing it exempts is itself denied.

**Statement 3 — agent-initiated mutation** (`DenyMutationViaMCP`, applied 2026-10-22). Denies the destructive action set outright when `aws:ViaAWSMCPService` is true — the condition key AWS sets on requests arriving through an AWS-managed MCP server. It reads differently from statements 1 and 2 on purpose: it is **not** scoped to the prod prefix, because it protects the whole account from an agent rather than prod from a developer, and it denies `iam:*` rather than the narrow prod-role set, because an agent able to edit IAM can remove the statement.

Its limitation is the thing to hold onto: the condition key exists **only** on MCP-routed requests. An `aws` CLI call, an SDK call or a `terraform apply` from the same session carries no such key and is entirely unaffected. Statement 3 does not make the account read-only for an agent; it makes *MCP-routed* access read-only, which is why the repo also carries a client-side `--read-only` proxy flag and a `PreToolUse` Bash guard. The three layers and what each one misses are catalogued in `.claude/skills/aws-account-operations/references/mcp-and-guardrails.md`.

The same change adds a fourth permission set, **`PlatformReadOnly`** (account-wide `ReadOnlyAccess`, PT8H, assigned to `developers`), so the MCP proxy can run as a read-only identity rather than as `PlatformAdmin`. `ProdDataReadOnly` cannot serve that purpose — its five Allow statements cover the lake, `rds-db:connect`, `eks:Describe*` and logs, so it cannot list buckets or read Budgets.

**The enforcement is weaker than the SCP version, materially** (full argument: D-ACCT): an inline deny *is* the identity-policy layer, so whoever can edit permission sets can remove the guard — one policy edit, not a structural boundary. The mitigations the claim depends on: the permission sets live in Terraform behind `main`'s branch protection (§8.6), so removing a deny is a reviewed, logged diff; `ProdAdmin` assumption is alerted on (§8.6); and the §17 probes assert the deny itself, not the mechanism. The Terraform apply role walks through the guardrail legitimately, so its **trust policy** is part of this claim — §14.2 pins it to the CI apply role on `main` and `ProdAdmin` as break-glass.

### 8.3 Customer-managed KMS keys, from day one

Every S3 bucket and every Aurora cluster gets a customer-managed key, for two specific reasons: an RDS snapshot encrypted with the default `aws/rds` key **cannot be shared across accounts** at all, and a dev principal cannot read an SSE-KMS prod object unless the prod key policy grants it. Retrofitting a CMK onto a live prod database means a full snapshot-copy-and-restore migration with downtime; doing it at creation costs nothing. The prod lake key grants `kms:Decrypt` to the dev refresh role; the prod Aurora key grants `kms:Decrypt` and `kms:CreateGrant` so a shared snapshot can be restored.

ECR is the deliberate exception — AES256, because images are built from public source, hold no records, and a CMK would require a key plus cross-account grants for no confidentiality gain (D-KMS).

### 8.4 How a dev workload reads prod

Humans assume `ProdDataReadOnly` through Identity Center. A *workload* needs an explicit role chain — same account since rev 4, but the two-role shape is kept so the readable surface is one role's policy, not the workload's:

```
IRSA role er-dev-data-refresh
  └─ sts:AssumeRole ──→ role er-prod-data-export
                               s3:Get*/List* on er-prod-lake
                               kms:Decrypt on the prod lake CMK
                               rds-db:connect as er_readonly
```

The export role's trust policy names the refresh IRSA role ARN specifically. It holds no write permissions, so §8.2 never comes into play.

### 8.5 Consequences of holding real data in lower environments

Lower environments hold real production records by design, protected by IAM and encryption rather than scrubbing. Three things follow.

**Committed dev credentials stop being harmless.** [frontend/dev/env.sh](../frontend/dev/env.sh) hardcodes `ERWEB_SESSION_SECRET`, `ERWEB_CREDENTIAL_KEY`, `ERSERVER_OPERATOR_TOKEN`, and the MinIO and Postgres passwords; `admin@dupezone.com / asdfasdf` is seeded by [frontend/dev/create-users.mjs](../frontend/dev/create-users.mjs). Appropriate for a MinIO-backed laptop stack; not for a cluster namespace holding real records. **The rule:** that set stays exclusively in the local Compose path, and every cloud environment — including each developer's namespace — draws all of them from Secrets Manager.

**NetworkPolicy must be explicitly enabled to do anything.** The per-namespace NetworkPolicy in §12 is inert under the Amazon VPC CNI unless the network policy agent is turned on in the addon configuration. Without it you get objects that look like isolation and enforce nothing — between namespaces that now hold real records. This is a one-line addon setting and a silent failure if missed, which is why §17 verifies it behaviourally.

**Attribution has two layers.** `X-Acting-User` is spoofable by any API-key holder by design, so person-level attribution comes from `erweb.audit`, not erserver's `audit_log`; CloudTrail data events on the prod lake (§7.3) are the backstop for data access. (One note: `ERWEB_SESSION_SECRET`, despite the name, is never read by any code — sessions are DB-backed opaque tokens hashed at rest. It needs a value to pass validation; it does not need a rotation policy.)

### 8.6 SOC 2 readiness

A first SOC 2 is expected ([backend-design.md](backend-design.md) §15 phase 4 names "SOC2 groundwork"), so the question is what to do *now* that cannot be done later. **SOC 2 Type 2 rarely fails on architecture; it fails on evidence** — controls must be shown to have *operated* across an observation window of three to six months. Almost everything can be added the week before an audit. Three things cannot, because they have a time component.

#### Cannot be retrofitted — build in Phase 0

**1. Retention must span the observation window.** Audit-relevant trails need **≥ 400 days**; debug logs keep the 30-day cost control (the split: D-RETENTION).

| Trail | Retention | Why it is audit-relevant |
|---|---|---|
| CloudTrail (management + data events) | 400 days | The primary evidence for CC6 access control and CC7 monitoring |
| `erweb.audit` (person-level) | 400 days | Who did what in the application — the only person-level attribution there is (§8.5) |
| erserver `audit_log` (credential-level) | 400 days | API-key-level actions |
| Application debug logs | 30 days | Not audit evidence. Keep the cost control here |

**2. Audit logs must land where no one with workload access can delete them.** In the single account there is no "elsewhere" to deliver to, so the control is carried by the bucket itself: versioning, **S3 Object Lock in compliance mode at 400 days** — a retention not even the root user can shorten — plus a policy denying deletes to everything but the account root. A developer can see the archive; nobody, including the principals the logs record, can remove an object before its retention expires. (A dedicated log-archive account is the first thing to reinstate if the account split returns — §3.)

**3. CloudTrail log file validation, enabled from the start.** CloudTrail writes signed digest files so you can prove the log was not altered. It only validates logs written *after* it is enabled.

**4. Access-change history, free from this design.** Identity Center group membership lives in Terraform (§8.1), so the quarterly access review SOC 2 CC6.2 expects is a git log plus PR approvals — provided `main` has branch protection with required review, itself a CC8 control. Both should be true before the window opens.

**Defer until an auditor is engaged** (point-in-time artifacts): written policies, risk assessment, vendor assessments, training records, IR runbooks, penetration testing. **Already satisfied:** CC6 (no human IAM users, no long-lived keys, MFA, short sessions, least privilege, §8.2 prod-mutation denies — IAM-layer since rev 4, §3), encryption at rest (§8.3), TLS by bucket policy (§7.3), CC8 change management (Terraform in git, tests on every PR, §11 promote-by-digest).

#### Findings today

- **Committed development credentials** (§8.5) — the first thing an auditor greps for; the §8.5 rule (local Compose only) is what makes it defensible, and it must be true in fact.
- **The AI assistant is an undocumented subprocessor.** [frontend/src/lib/assistant/tools.ts](../frontend/src/lib/assistant/tools.ts) gives the model `search_golden_records` and `get_entity`, so a shipped feature sends real customer personal data to the Anthropic API via `ANTHROPIC_API_KEY`. It needs inventorying, a DPA, a configured vendor-side retention posture, and customer disclosure (§18.14).
- **Standing production read access.** §8.1 grants every developer `rds-db:connect` as `er_readonly` plus prod-lake read — defensible under SOC 2 only with a documented business justification and CloudTrail data events attributing every read (§7.3). `ProdAdmin` on one person is weak segregation of duties, so **alert on `ProdAdmin` role assumption** plus a documented break-glass procedure (§15, Phase 4).

**Evidence generators worth enabling:** AWS Config (narrow scope, a few $/mo — has a time component, so Phase 0), GuardDuty prod-only (~$5–30/mo), Security Hub with the AICPA SOC 2 standard (~$10/mo, premature now).

## 9. Secrets

[server/src/erserver/secrets.py](../server/src/erserver/secrets.py) already resolves an org env value written `secret://NAME` from `ERSERVER_SECRET_<NAME>` in the server's own process environment (`_ENV_PREFIX = "ERSERVER_SECRET_"`), storing only the reference in the control-plane database so a DSN never rides in a job row. That is the seam, and it means secrets *delivery* needs no application change.

External Secrets Operator, authenticated by IRSA, projects Secrets Manager entries into Kubernetes Secrets consumed via `envFrom`:

```
er/{env}/{namespace}/erserver
  ERSERVER_DSN, ERSERVER_MAINT_DSN, ERSERVER_TENANT_DSN_TEMPLATE,
  ERSERVER_OPERATOR_TOKEN, ERSERVER_SECRET_S3_KEY, ERSERVER_SECRET_S3_SECRET,
  ERSERVER_SMTP_URL, ERSERVER_TENANT_ENV_JSON
er/{env}/{namespace}/erweb
  ERWEB_DATABASE_URL, ERWEB_SESSION_SECRET, ERWEB_CREDENTIAL_KEY, ANTHROPIC_API_KEY,
  ERWEB_OIDC_{GOOGLE,MICROSOFT,SALESFORCE}_{CLIENT_ID,CLIENT_SECRET}   ← frontend-design §2.4
```

### 9.1 Rotation needs a restart, which `envFrom` does not give you

A process environment is fixed at exec time, so on rotation ESO updates the Kubernetes Secret while the running pods keep the stale DSN and connections fail (why managed rotation alone breaks: D-ROTATION). Two parts to the fix. A **reloader** — annotate the Deployments so a change to the projected Secret triggers a rolling restart. And for the dispatcher, which is `replicas: 1` / `strategy: Recreate` (§2), rotation is scheduled rather than arbitrary, since a restart is a brief gap in dispatch.

**`ERWEB_CREDENTIAL_KEY` must not be rotated automatically at all.** It unseals every org's stored erserver API key in `erweb.org_credentials` (AES-256-GCM with AAD `"<org>\0<role>"`, per [frontend/src/lib/db/crypto.ts](../frontend/src/lib/db/crypto.ts)); rotating it without a re-encrypt pass bricks the vault. Documented as manual-with-migration, deliberately not wired to a schedule.

### 9.2 S3 credentials: static keys now, IRSA later

`ObjectStore.from_env()` in [src/er/lake/objectstore.py](../src/er/lake/objectstore.py) and `attach_statements()` in [src/er/lake/ducklake.py](../src/er/lake/ducklake.py) both require a literal `ER_S3_ACCESS_KEY_ID` / `ER_S3_SECRET_ACCESS_KEY` and a non-empty scheme-less `ER_S3_ENDPOINT`; there is no credential-chain path and no way to omit the endpoint (`require_env` rejects empty values, since an empty secret attaches a lake that authenticates as nobody).

**Phase 1 (no code change).** One IAM user per environment with a lake-scoped policy, keys in Secrets Manager, with `ER_S3_ENDPOINT=s3.us-east-2.amazonaws.com`, `ER_S3_URL_STYLE=vhost`, `ER_S3_USE_SSL=true`. This works against real S3 today; rotation has the §9.1 restart requirement.

**Phase 4 (the right answer).** A credential-provider mode so DuckDB uses `CREATE SECRET (TYPE s3, PROVIDER credential_chain)` and boto3 uses its default chain, both picking up the IRSA web-identity token, after which no static S3 key exists anywhere. Scheduled separately because it touches [src/er/lake/ducklake.py](../src/er/lake/ducklake.py), [src/er/lake/objectstore.py](../src/er/lake/objectstore.py), and [dbt/profiles/profiles.yml](../dbt/profiles/profiles.yml) together, and `tests/unit/test_dbt_profiles.py` contract-tests those three as field-for-field identical.

### 9.3 SES: the second static credential, retired before it was created

**This section used to specify a second IAM user. It no longer exists, and never did.**

The reasoning that required it still holds: SES's SMTP interface **does not accept credentials derived from temporary credentials**, so no amount of §9.2 progress could have put the SMTP path behind Pod Identity (D-SES). A `ses:SendRawEmail`-only IAM user with its SMTP password in Secrets Manager was the correct answer *for a mailer that speaks SMTP* — which the Python one did, via stdlib `smtplib` behind `ERSERVER_SMTP_URL`.

What changed is that the Python mailer was deleted with the rest of the Python server, so the Rust one was written fresh rather than ported. At that point the Phase 4 plan — "port the mailer to the SES API, retire the key" — was cheaper to do than the thing it was scheduled to undo. `er_server::mail` calls `SendEmail` through the SES API and signs with the pod's EKS Pod Identity credentials, so:

- No IAM user, no access key, no SMTP password, no Secrets Manager entry, nothing in §9.1's rotation scope.
- §8.1's "one long-lived key" claim needs no amendment for email. §9.2's S3 user is the only exception left, and the only item remaining in Phase 4's credential work.
- `ERSERVER_SMTP_URL` is gone from `scripts/dev_env.sh`. It had already been dead for a while: the secret was still written, but no Rust code read it.

The cost is a dependency, not a credential: `aws-config` + `aws-sdk-sesv2` enter er-server's build graph. Both are pinned `default-features = false` to exclude aws-lc-rs (its `-sys` crate needs cmake the `rust:1-bookworm` build image lacks) and the SSO credential providers (which require Rust 1.88 and would silently raise the declared MSRV of 1.85). The consequence to know: `aws sso login` credentials do **not** work for a locally-run er-server, which is why dev uses `ER_MAIL_BACKEND=log`.

## 10. Observability

Nothing currently leaves the box — no metrics endpoint, no tracing, no log aggregation. [backend-design.md](backend-design.md) §14 specifies what should exist (pod logs tagged `tenant/job/run/stage`, API RED metrics, queue depth, dispatch latency, per-class concurrency) and none of it is wired, mostly because there has been no cluster to wire it to. **OpenTelemetry to one collector** is the foundation, exporter swappable (the four system-specific reasons, and the one-collector-but-not-one-backend split, are in D-OTEL).

### 10.1 What already exists, and the boundary not to cross

| Exists | Where | Role |
|---|---|---|
| `runs`, `run_stages` with typed counters and snapshot ranges | [src/er/obs/runctx.py](../src/er/obs/runctx.py) | **Durable business telemetry, already in Postgres** |
| One JSON record per stage on stderr | engine S5.2 records | Live progress, consumed into `jobs.progress` |
| Opt-in profiling: `ER_PROFILE_SQL`, `ER_PROFILE_PYTHON`, `ER_PROFILE_STORAGE` → `ER_PROFILE_DIR` | [src/er/obs/sql_profile.py](../src/er/obs/sql_profile.py), [docs/profiling.md](profiling.md) | Deep per-query and per-stage attribution, off by default |

**`runs`/`run_stages` stays the system of record for business counters.** OTel metrics are for *operational* questions. The rule: anything a user might see in the UI comes from Postgres; anything an operator pages on comes from OTel.

### 10.2 Traces — and the one schema change that makes them work

The runner is a separate pod launched asynchronously from a database row (§6), so trace context has to survive that hop; it does not travel by itself. **Add `jobs.trace_context` (text, nullable)** holding the W3C `traceparent` captured at enqueue, which the runner reads to start its root span as a child of that context. This is the highest-value single decision in this section: with it, one trace spans the click, the API call, the queue wait, the dispatch, every pipeline stage, and the dbt invocations. Without it you get two unrelated traces and a `run_id` to join them by hand.

Instrumentation, in rough order of value per unit of effort:

- `opentelemetry-instrumentation-fastapi` and `-psycopg` on the API and dispatcher — near-zero effort, immediately gives RED metrics and DB span timing.
- **A span per pipeline stage** in the runner. [src/er/obs/runctx.py](../src/er/obs/runctx.py) already brackets exactly these boundaries and holds the counters that become span attributes, so this is an adapter rather than new instrumentation.
- `@vercel/otel` in the BFF, propagating into the typed erserver client.
- The queue wait as its own span — the gap between enqueue and claim is where per-tenant serialization shows up, invisible today.

Sampling: **always sample pipeline runs** (hundreds per day, each high-value); head-sample the API read path, where the volume is. **One gotcha specific to Jobs:** the OTel SDK's batching exporter drops unflushed spans when a process exits, and a runner pod exits immediately after its final span — so the tail of every run, including the failure path, would silently vanish. The runner needs an explicit `force_flush()` with a timeout in its exit path, on every exit code.

### 10.3 Metrics — and the cardinality trap

[backend-design.md](backend-design.md) §14's list maps cleanly: API RED, queue depth, dispatch latency, per-class concurrency. Worth adding, because they validate claims this document cannot currently check: **spill bytes per stage** (§18.2 needs this anyway), **pod eviction and OOM-kill counts** (how §5.3's sizing proves correct), **spot interruption rate and Karpenter node count** (how §5.2's suspend and §16's spot pricing get verified). **Do not put `tenant` on metric series** — every distinct label value is a separate time series, tenant count is unbounded by design, and the bill scales with tenants × metrics × resolution. Use `resource_class` as the metric dimension and get per-tenant detail from traces and the `runs` table; the engine's own `tenant` tagging is correct for logs and spans and wrong for counters.

### 10.4 Logs, and the PII rule

The engine already writes structured JSON to stderr; add `trace_id` and `span_id` so a log line links to its span. **This is a deduplication engine, so log content is a privacy question:** names, emails, phones, and addresses are the payload. The rule — **log lines carry identifiers and counts (`record_key`, `entity_id`, `tenant`, row counts), never attribute values** — is the control (a redaction processor in the collector is only a backstop), and §17 verifies it. Volume is a real cost: dbt is chatty, a 10M-row run's dbt output alone can reach hundreds of megabytes at ~$0.50/GB, so log level is environment-configured (dbt at `warn` in prod, `info` in dev). **Retention is split by purpose** — debug output 30 days, audit trails 400 days for CloudTrail, `erweb.audit`, and erserver's `audit_log` (§8.6, D-RETENTION).

### 10.5 One pipe, one destination to start, split by signal later

**One collection layer, unambiguously:** every deployable speaks OTLP to one collector, which batches, redacts, samples, and routes — the one decision embedded in application code. Topology: an **OTel Collector Deployment (gateway)** for traces and metrics (pushed over OTLP, `force_flush()` on exit), plus a **DaemonSet** whose `filelog` receiver collects logs from stdout/stderr — the reliable path for an ephemeral Job, because the runtime captures both streams however the process exits, so logs survive exactly the crash that loses unflushed OTLP spans. **Start with CloudWatch and X-Ray via ADOT** — not the best backend, but the exporter is swappable and the cost at one developer is a few dollars. CloudWatch Logs at ~$0.50/GB is a rounding error at one developer and a top-three line item at a hundred tenants (§16); the known next step is splitting by signal — logs stay in CloudWatch (in-account, where the PII boundary requires them), metrics and traces to Grafana Cloud or AMP+Grafana. (Self-hosted LGTM and deriving metrics from logs were rejected — D-OTEL.)

### 10.6 Phasing, and the coupling to §6

- **Phase 1:** collector deployed; logs shipped with trace fields; FastAPI/psycopg auto-instrumentation. Cheap, immediate, no schema change.
- **Phase 2, alongside the Job launcher:** `jobs.trace_context`, spans per stage, `force_flush()` on runner exit. **Designed together with §6.3's progress decision** — both cross the dispatcher-to-pod boundary the subprocess pipe used to cover.
- **Phase 4:** `ER_PROFILE_*` output as span events, dashboards for the §14 metric list, alerting.

## 11. Container registries

Three images, two of which do not exist yet:

| Image | Status | Contents |
|---|---|---|
| `er-pipeline` | exists — [docker/Dockerfile](../docker/Dockerfile) | The runner. Baked DuckDB extensions, dbt, and compiled MinIO binaries |
| `er-api` | **new** | Slim: `src/er` + `server/src/erserver`, no dbt, no MinIO. Serves both `er-api` and `er-dispatcher` |
| `er-web` | **new** | Next.js standalone output on Node 22 |

ECR repositories live in the account's single registry, with tag immutability, scan-on-push, and a lifecycle policy retaining the last 30 images. **Promotion is by digest, never by tag:** a prod deploy pins the exact digest CI built and tested, which proves what ran in prod came from a specific reviewed commit (the dropped cross-account replication arrangement is in D-REGISTRY). CI gains a GitHub OIDC provider and a push job replacing today's `load: true`: it currently consumes zero repository secrets and declares only `permissions: contents: read`, and the push job adds `id-token: write` and assumes `er-ci-ecr-push`, whose trust policy restricts the `sub` claim to pushes on `main` — **pull requests are excluded deliberately**, since a PR from a fork runs attacker-authored workflow code. (`er-pipeline` shipping a compiled MinIO *server* binary is worth revisiting as avoidable attack surface.)

### 11.1 Cold nodes pull the whole image, every time

For the runner pools there is no node cache: §5.1 empties a runner node ~30s after its pod exits, so nearly every run starts on a freshly provisioned node pulling `er-pipeline` from scratch (the "let the node cache do the work" plan was defeated — D-PULL). The bytes are cheap (layer blobs ride the gateway endpoint, §4) but the minutes are real on an image carrying a full venv, baked extensions, compiled MinIO binaries, and the tests/fixtures/dev group. The mitigation ladder, cheapest first:

1. **SOCI parallel pull — effectively free.** Ships in current EKS AL2023 and Bottlerocket AMIs, on by default under Auto Mode, up to ~60% faster pulls on 10 GB-class images.
2. **A runtime/CI image split.** The runtime target drops `tests/`, `fixtures/`, `benchmarks/`, the dev group, and the MinIO server binary. This is a contract change: `test_dockerfile_contract.py` must move with it.
3. **Snapshot-prebaked images.** Standard Karpenter only — unavailable under Auto Mode (§5.5) — and it adds an artifact to keep fresh, so it is the last rung.

The deciding measurement, cold-node pull time for the current image, does not exist yet (§18.16).

## 12. Per-developer environments

A developer's environment is a namespace plus its own Postgres and S3 prefix, driven by Helm so it takes seconds and holds no Terraform state:

```
make dev-up DEV=alice
  1. helm upgrade --install alice charts/er-dev-namespace \
       --set developer=alice --set lakePrefix=dev/alice
     → Namespace, ResourceQuota, LimitRange, NetworkPolicy,
       Postgres StatefulSet + PVC, EFS PVC, IRSA ServiceAccount
       scoped to s3://er-dev-lake/dev/alice/*, ExternalSecret,
       three Deployments, Ingress at alice.dev.<domain>
  2. wait for postgres-0, then CREATE DATABASE erctl (via er_provisioner)
  3. drizzle migrate (hook) + er-server migrate (init container) + migrate-tenants (post-upgrade hook)
  4. seed
```

Secrets come from `er/dev/alice/{erserver,erweb}`, created by the same target if absent.

Three details carry over from the local stack. The dispatcher gets `replicas: 1`, `strategy: Recreate`, and an init container gating on the API's `/healthz`, because its leader connection otherwise blocks `ensure_schema` indefinitely. The Postgres pod runs with `-c max_locks_per_transaction=1024`, matching both compose files. And `ResourceQuota` is what stops one developer's 10M-row run from starving the cluster — `requests.memory: 80Gi`, `limits.cpu: 24`, `count/jobs.batch: 4`, and **`requests.ephemeral-storage`**, without which §5.3's spill sizing is unenforced.

Two details are cloud-only. The per-developer Ingresses share **one internal ALB** via the controller's `group.name` (§4.1) — the controller's documented warning that anyone with Ingress RBAC in the group can override its rules describes, in dev, people who already hold `PlatformAdmin`, which is why prod never joins a dev group. external-dns publishes `alice.dev.<domain>` against the wildcard certificate, scoped with `--domain-filter=dev.<domain>` and a per-cluster `--txt-owner-id`. Step 3's drizzle migrations run as a Helm pre-upgrade hook Job (`node scripts/db-migrate.mjs`, reading `ERWEB_DATABASE_URL`); the Rust control-plane schema reconciles in the web-api pod's init container (`er-server migrate`: the baseline plus the catalog reconciler, ledgered in `public.schema_migrations`) and again, idempotently, at the API's boot; and every active tenant's lake is reconciled by the post-upgrade hook (`er-server migrate-tenants`, one line per tenant) with a daily CronJob repeating it as drift detection. See [entity-framework-design.md](entity-framework-design.md), "Upgrades and API versions".

Lifecycle:

```
make dev-suspend DEV=alice   # Deployments + StatefulSet → 0. PVC and S3 kept.
make dev-resume  DEV=alice   # back up in under a minute
make dev-down    DEV=alice   # + delete PVC, DROP DATABASE ... WITH (FORCE),
                             #   delete the S3 prefix, delete the secrets
```

Suspend is the cost control (§5.2) and `dev-down` is the purge path [backend-design.md](backend-design.md) §7 designed for. The distinction matters: suspend keeps the catalog, so resuming does not mean another refresh.

## 13. Moving production data to lower environments

Lower environments take real records with no scrub step. The constraint is that a **full-fidelity lake clone is not supported by the engine today**: `er init` records `DATA_PATH` in the catalog and the immutability check at [src/er/lake/init.py:180](../src/er/lake/init.py) raises `PreconditionFailure` (exit 3) on mismatch. Restoring a prod catalog into a dev namespace yields a catalog still pointing at `s3://er-prod-lake/...`, and no supported operation re-homes it.

**Path A — export and re-ingest.** What this design builds.

```
make refresh-dev DEV=alice ORG=acme
  1. assume prod/er-data-export
  2. read prod source + golden records via DuckDB over S3 → Parquet
  3. write to s3://er-dev-lake/dev/alice/_import/
  4. er ingest in Alice's namespace, then run_all_full
```

Full raw fidelity on the records, using only supported engine paths. It loses prod's snapshot history and costs a pipeline run — roughly 90s at 1M records, or about 1h04m at 10M with 6 threads ([performance.md](performance.md)). Because dev Postgres is a pod rather than Aurora (§7.1), the catalog side of a refresh is `pg_dump`/`pg_restore` rather than a snapshot share.

**Path B — `er lake rehome`.** A new engine command that updates the recorded `DATA_PATH` and rewrites catalog file references, reducing refresh to an `aws s3 sync` plus a catalog restore: minutes instead of an hour, with snapshot history preserved. Raised as its own issue, because the work depends on whether DuckLake stores absolute or relative paths in the catalog — worth determining early, since it decides whether Path A is permanent (§18.7). Cross-account Aurora snapshot plumbing is still built for staging refresh and prod DR, just not for the per-developer path.

## 14. Repository layout

```
infra/
  terraform/
    bootstrap/
      organization/       the Organization import + the parked former-nonprod account
      state-backend/      the S3 state bucket
    modules/
      identity-center/    permission sets (incl. the §8.2 guard denies), groups, assignments
      github-oidc/        OIDC provider + CI push role
      network/            VPC, subnets, S3 gateway endpoint, NAT, Tailscale subnet router
      eks/                cluster, addons, Karpenter NodePools, access entries
      aurora/             Serverless v2, parameter group, CMK, roles, snapshot share
      lake/               S3 bucket, CMK, lifecycle, scoped CloudTrail
      efs/                filesystem + access points
      ses/                domain identity, Easy DKIM, MAIL FROM, configuration set
      irsa/               reusable IRSA role factory
    envs/
      core/               the account: billing, identity, audit, registry, CI trust
                          (dev/prod workload stacks arrive with Phases 1 and 3)
  k8s/
    charts/
      er-platform/        environment-level: api, dispatcher, web, ingress
      er-dev-namespace/   per-developer namespace, including its Postgres
```

Bootstrap is two stacks, not one, because the state bucket must live in an account and a provider that assumes into an account created by the same apply has an unknown `role_arn` at plan time. Terraform state lives in S3 in the account with `use_lockfile = true` — native conditional-write locking, no DynamoDB table. The bucket is versioned (the only recovery path from a truncated state file) and SSE-KMS encrypted under its own CMK, because state holds database endpoints and every output marked sensitive. State, like everything else, is protected by role policy rather than an account boundary (§3).

### 14.1 Application changes this requires

| Change | Files | Why |
|---|---|---|
| `er-api` image | new `docker/Dockerfile.api` | Slim, no dbt or MinIO; [backend-design.md](backend-design.md) §4 specifies it |
| `er-web` image | new `frontend/Dockerfile` | None exists; the BFF runs as a host process today |
| `ERSERVER_TENANT_DB_PREFIX` | [server/src/erserver/provision.py](../server/src/erserver/provision.py), [settings.py](../server/src/erserver/settings.py) | Shared-cluster tenant isolation (§7.2) |
| CI push job | [.github/workflows/ci.yaml](../.github/workflows/ci.yaml) | OIDC + `push: true`, adding `id-token: write` |
| Normalize the lake prefix | [configs/default.yaml](../configs/default.yaml), [frontend/dev/env.sh](../frontend/dev/env.sh) | Three inconsistent layouts today (§7.3) |
| **k8s Job launcher** behind the existing seam | `launch_runner` in [server/src/erserver/dispatcher.py](../server/src/erserver/dispatcher.py) | Replaces `subprocess.Popen` with a Job + watch (§6.2) |
| **Runner writes its own progress** | [server/src/erserver/runner.py](../server/src/erserver/runner.py), [queue.py](../server/src/erserver/queue.py) | A Job pod has no stderr pipe to the dispatcher (§6.3) |
| **Per-run resource sizing** from record count | [server/src/erserver/queue.py](../server/src/erserver/queue.py), [policy.py](../server/src/erserver/policy.py), derived from [benchmarks/scales.yaml](../benchmarks/scales.yaml) | CPU-tiered classes on the job row, not `orgs.env` (§6.5) |
| **`jobs.trace_context`** column | [server/src/erserver/db.py](../server/src/erserver/db.py) | W3C traceparent across the enqueue→pod hop; one trace end to end (§10.2) |
| **OTel instrumentation + `force_flush()` on exit** | [server/src/erserver/api.py](../server/src/erserver/api.py), [runner.py](../server/src/erserver/runner.py), [src/er/obs/runctx.py](../src/er/obs/runctx.py), `frontend/` | Spans per stage; an exiting Job otherwise drops its last batch (§10.2) |
| **Per-run cost on the run row** | [server/src/erserver/queue.py](../server/src/erserver/queue.py), [src/er/obs/runctx.py](../src/er/obs/runctx.py) | vCPU-hours × node rate, recorded next to the existing counters (§16.2) |
| Non-root `USER` in all three images | [docker/Dockerfile](../docker/Dockerfile) (none today), both new Dockerfiles | Pod Security `restricted` requires it (§5.6) |
| Disable FastAPI docs routes outside dev | [server/src/erserver/api.py](../server/src/erserver/api.py) | `/docs`, `/redoc`, `/openapi.json` are the only unauthenticated routes besides `/healthz` (§4.1) |
| *(optional)* `api_keys.last_used_at` | [server/src/erserver/db.py](../server/src/erserver/db.py), [auth.py](../server/src/erserver/auth.py) | Turns the SOC 2 credential review into a query (§8.6) |
| *(Phase 4)* S3 credential chain | [src/er/lake/ducklake.py](../src/er/lake/ducklake.py), [objectstore.py](../src/er/lake/objectstore.py), [dbt/profiles/profiles.yml](../dbt/profiles/profiles.yml) | Drop static keys for IRSA; contract-tested together (§9.2) |

Most of the additions come from one source: putting the runner in its own pod removes the stderr pipe the dispatcher currently relies on, which pulls in progress reporting, trace propagation, and flush-on-exit together. They should land as one piece of work (§10.6).

### 14.2 How a change reaches an environment

**Terraform.** Plan on every PR under a read-only role; apply on `main` under the apply role — both assumed through §11's GitHub OIDC provider, with the same `main`-only `sub` restriction the ECR push role carries. The prod-scoped apply role's trust policy names exactly two principals — the CI apply role and `ProdAdmin` as break-glass. That trust policy is the §8.2 guardrail's last clause.

**Helm.** Dev platform releases apply from CI on `main`; `make dev-up` stays a human command. Prod deploys are manually triggered, pass an image **digest** — never a tag — and gate on that digest being present in the registry first. This is the minimum that turns promote-by-digest from a convention into a property (why it is load-bearing: D-DEPLOY). Argo CD remains out of scope (§6.1).

## 15. Build order

The live tracker with acceptance criteria is [plan-to-aws.md](plan-to-aws.md); this is the phase skeleton.

**Phase 0 — Foundations, and everything that cannot be retrofitted.** The account baseline (root MFA, S3 BPA, billing access), Identity Center permission sets and groups (including the §8.2 guard denies), the two bootstrap stacks, GitHub OIDC, ECR repositories, budget alarms. Plus the time-component controls, the only reason they are this early: cost attribution (§16.1 — tags activated, EKS split cost allocation, CUR delivery; none backfills); audit trails (§8.6 — CloudTrail with log file validation to the compliance-mode Object-Locked bucket at 400 days, AWS Config narrow scope); branch protection on `main`; and the account floors (S3 BPA on the account, MFA on the root user, no root access keys). All verifiable with no cluster running.

**Phase 1 — dev.** Network, EKS dev, Auto Mode with the §5.1 disruption settings and §5.6 hardening (the §5.5 Auto Mode check happens here, before the configuration deepens), the dev lake bucket with lifecycle, EFS with automatic backups, External Secrets Operator with a reloader. Both Dockerfiles and the CI push job. Ship `ERSERVER_TENANT_DB_PREFIX`. The edge and email substrate lands here (lead time or blocks Phase 2): Cloudflare DNS wiring (§18.8), wildcard dev certificate, external-dns, the internal ALB and Tailscale subnet router (§4.1); the SES module applied (the DKIM/MAIL FROM records are Terraform's now — `cloudflare_dns_record` in `envs/dev/ses.tf`) and the production-access request filed (§7.5) — the module, the records and the sender are written, so production access is the manual remainder. Plus the observability floor (§10.6): collector gateway and log DaemonSet, logs with trace fields, FastAPI/psycopg auto-instrumentation.

**Phase 2 — Per-developer environments, and the Job launcher.** The `er-dev-namespace` chart including its Postgres StatefulSet, the erweb migration hook Job, the shared-ALB Ingress annotations (§12); `make dev-up`/`suspend`/`resume`/`down`, ResourceQuota with ephemeral-storage, the runner NodePools with NVMe, and the idle-suspend CronJob with the §5.2 predicate. This is where the runner becomes a Kubernetes Job (§6) and carries the coupled set — progress written by the runner (§6.3), `jobs.trace_context` and per-stage spans (§10.2), `force_flush()` on exit, per-run CPU sizing (§6.5), and the EFS mounts on the Job and dispatcher (§6.2) — as one piece of work.

**Phase 3 — Prod, when there is prod traffic.** Network, EKS prod, Aurora prod multi-AZ, the prod lake with CMK and scoped CloudTrail, the prefix-scoped export role (§8.4), `make refresh-dev` Path A, and the internet-facing prod ALB behind WAF with rate rules on the three unauthenticated write paths (§4.1).

**Phase 4 — Hardening and the audit run-up.** The IRSA credential chain (§9.2) — the SES API port that was to retire the SMTP key is **done**, and the key was never created (§9.3) — per-tenant scoped ServiceAccounts (§6.4), `ER_PROFILE_*` as span events, dashboards for the [backend-design.md](backend-design.md) §14 metric list, alerting, automated snapshot sharing, GuardDuty in prod, and the point-in-time SOC 2 artifacts (policies, risk assessment, subprocessor inventory including the Anthropic flow, vendor assessments, break-glass procedure, a restore test with its record — §8.6).

*A follow-up worth noting:* an EKS cluster unlocks the GitHub Actions Runner Controller, which resolves a recorded blocker — [benchmarks/scales.yaml](../benchmarks/scales.yaml) asks for `ubuntu-latest-8-cores` and `ubuntu-latest-16-cores` at the `100k` and `1m` scales, neither available today. A `runner-l` node serves them natively.

## 16. Cost

Rough `us-east-2` monthly figures (identical to `us-east-1` for every line item here). Estimates for shaping decisions, not a quote. (The first draft was optimistic by ~2×; the correction story is D-COST2X.)

| Item | Dev | Prod (deferred) |
|---|---|---|
| EKS control plane | $73 | $73 |
| System node group (2 × `t4g.medium`) | $24 | $24 |
| App tier — spot, suspended when idle | ~$8 | ~$60 |
| Runner nodes — spot `c6id`, scale to zero | ~$7 | ~$18 |
| Postgres | **~$4** (gp3 PVC) | ~$50 (Aurora) |
| NAT Gateway | $33 | $99 (3 AZ) |
| ALB (internal in dev) | $16 | $16 |
| Tailscale subnet router (`t4g.nano`) | ~$4 | — |
| WAF on the prod ALB | — | ~$8 |
| EFS + S3 + ECR | ~$10 | ~$25 |
| Observability — collector + CloudWatch/X-Ray | ~$8 | ~$25, log-volume driven (§18.4) |
| Audit trails + Config (§8.6) | ~$5 | ~$15 |
| CloudTrail data events (scoped) | — | ~$5–50, unmeasured (§18.4) |
| **Subtotal** | **~$192** | **~$418** |

The runner line dropped despite more vCPU: §6.5's switch from `r6id` to `c6id` buys the same 16 vCPU and NVMe for about a third less. Because prod is deferred (§15), the real near-term number is the dev column: **roughly $190/mo**. The fixed floor is $73 control plane + $24 system nodes + $33 NAT + $16 ALB + $4 Tailscale = **$150**, 78% of it and none elastic; the remaining ~$42 is what suspend-on-idle moves. If $180/mo matters, the lever is not more autoscaling — it is dropping the ALB for `kubectl port-forward` (−$16), a public-subnet node group with no NAT (−$33), or questioning the managed control plane for one developer. Four line items are load-bearing assumptions: spot pricing on the runner pools (safe only because `dispose()` treats a killed runner as resumable), CloudTrail data events (§18.4), observability log volume (why §10.4 makes log level environment-configured), and lake storage (not estimable until a refresh has run). Observability is the one line that grows with *usage* rather than provisioning. Budget alarms at $300 and $600 are part of Phase 0.

### 16.1 Measuring actual spend, not just estimating it

Three mechanisms turn the estimate into measurement, and the first has a sequencing trap.

**Cost allocation tags.** Every resource carries `Project=er`, `Environment`, `ManagedBy`, and — on per-developer resources — `Developer`, via the provider's `default_tags`. These must be **activated as cost allocation tags in the Billing console, in Phase 0, before anything is created**: activation takes up to 24 hours and *does not backfill*. Activate them late and the first weeks of spend are permanently unattributable.

**Cost and Usage Report → Athena.** Anything programmatic — a per-tenant monthly figure, a cost-per-million-records trend — needs the CUR delivered to S3 and queried with Athena. Partition it, or Athena scans the whole history on every query.

**Split Cost Allocation Data for EKS.** A Kubernetes pod is not an AWS resource, so ordinary tags cannot see it; enabling split cost allocation adds per-pod rows to the CUR, dividing each instance's cost across the pods that ran on it, with namespace and workload as columns. Two payoffs fall straight out of design choices: **per-developer cost comes free** because §12 makes an environment a namespace, and **per-tenant compute cost is tractable** because §6 makes a run a dedicated single-tenant pod.

### 16.2 Per-tenant attribution: direct costs and an allocation basis

The shared portion is mostly *fixed*, and the variable portion is mostly *direct*:

| Cost | Attribution | Basis |
|---|---|---|
| **Runner pods** — the dominant variable cost | **Direct** | One Job, one tenant. CUR split rows by pod, or the internal model below |
| **Lake storage** | **Direct** | Per-tenant prefix (§7.3). S3 Inventory → Athena gives exact bytes per prefix |
| **Catalog storage** | **Direct** | `pg_database_size()` per tenant database — a dedicated DB per tenant (§7.1) makes this one query |
| **Lake request costs** | Direct but expensive to collect | S3 server access logs or CloudTrail data events grouped by prefix (§18.4) |
| `er-api`, `er-web`, `er-dispatcher` pods | **Shared — not attributable** | Allocate by share of runs, or leave as platform overhead |
| Aurora compute | **Shared** | Allocate by catalog size or run count |
| EKS control plane, NAT, ALB, system nodes, EFS | **Fixed overhead** | Do not allocate. §16's floor is ~$150 of this |

**Also compute per-run cost internally, in Postgres**, alongside the CUR. Every term is already recorded: `run_stages` has per-stage duration, §6.5 sets vCPU and memory, and the node's hourly price follows from its instance type. This is allocated cost at a blended rate, not billed cost (within ~10%, §18.12); the CUR stays the authority for the actual bill. (Kubecost/OpenCost were considered and deferred — D-COSTTOOL.)

### 16.3 Knowing when a tenant's cost means something is wrong

The purpose is narrow: **a signal for when the current design has stopped working for some customer shape, early enough to act.** The business half needs no new instrumentation — §10.1's `runs`/`run_stages` and the typed `PROMOTED_COUNTERS` ([src/er/obs/counters.py](../src/er/obs/counters.py)) are already written per stage per run. Joined to §16.2's per-run cost, three monthly questions (one Athena query over the CUR, one SQL query over `runs`/`run_stages`): (1) **is any tenant disproportionate** — cost per month normalized to **cost per million records**, where a *small* tenant costing like a big one is the signal; (2) **is cost per million stable, rising, or falling** — the degradation detector, most valuable because it catches problems while still cheap; (3) **how much is spent on runs that failed** — a run dying in the last stage costs nearly what success does, and a thrice-retried spot eviction costs triple (§5.3). A rising cost-per-million for one tenant is a config problem (blocking keys degrading — tune blocking, not instance types); rising across all tenants is systemic (snapshot retention not expiring — §7.3). Two alarms are worth automating: any single tenant exceeding a set share of variable spend, and failed-run spend exceeding a set share of total.

## 17. Verification

Each phase has a behavioural check, not a green `terraform apply`.

**Phase 0 — the security claim.** The §8.2 assertion, probed against the guard denies the permission sets carry (IAM-layer, so the probe asserts the deny itself). The prod lake does not exist until Phase 3, so Phase 0 asserts against the prod-prefixed names directly:

```bash
aws sso login --profile platform-admin
aws s3api list-objects-v2 --bucket er-prod-lake      # read path intact (NoSuchBucket until Phase 3 is fine)
aws s3api put-object --bucket er-prod-lake --key probe   # MUST fail: AccessDenied, Statement 1 (§8.2)
aws iam create-role --role-name er-prod-app-probe \
  --assume-role-policy-document file://trust.json    # MUST fail: Statement 2 (§8.2)
aws sso login --profile prod-admin
aws iam create-role --role-name er-prod-app-probe ... && aws iam delete-role --role-name er-prod-app-probe   # succeeds — the exemption works
```

The second and third commands failing under `PlatformAdmin` is the point; the third tests the privilege-escalation statement — if it succeeds, the `er-prod-*` exemption is an open door. Re-run against the real lake once Phase 3 lands, and **after any permission-set change**, because rev 4 moved the guard into exactly the layer such a change edits.

**Phase 0 — the things that cannot be checked later.** Cost attribution (§16.1): confirm the cost allocation tags show as *Active* in Billing and that a CUR file has landed with split cost allocation columns present. Audit trails (§8.6): confirm CloudTrail digest files are written and `aws cloudtrail validate-logs` passes; confirm the archive bucket's retention is 400 days and Object Lock is in compliance mode; then, as the control test, **assume `PlatformAdmin` and attempt to delete an object from the archive bucket — it must fail.** That is the property an auditor is checking: that the principals a log records cannot erase it.

**Phase 1 — image and registry.** Push from CI; confirm tag immutability rejects re-pushing an existing tag. Confirm a PR build cannot assume the push role.

**Phase 2 — a developer environment, and nine things likely to be silently wrong.**

```bash
make dev-up DEV=alice
# log in at alice.dev.<domain>, create an org, submit run_all_full
kubectl -n alice get jobs -w
kubectl -n alice exec deploy/er-api -- df -h /app/.tmp   # NVMe, not the root filesystem
```

- **The parameter actually took effect.** Run `er lake maintain`, the operation that fails at the default of 64 locks.
- **Suspend really reaches zero.** `make dev-suspend DEV=alice`, then confirm Karpenter reclaims the `app` node and no pod holds a database connection (§5.2).
- **The catalog survives suspension.** Suspend, resume, confirm golden-record counts and snapshot history unchanged. §7.1 rests on the PVC outliving the pod; a misconfigured volumeClaimTemplate silently gives an `emptyDir`.
- **NetworkPolicy enforces.** From Alice's namespace, attempt to reach Bob's Postgres. It must fail. Under the VPC CNI without the policy agent enabled, this *succeeds* while the object looks correct (§8.5).
- **Kubernetes does not retry the Job.** Submit a run with a deliberately invalid config (engine exit 2) and confirm exactly one pod is created. A default `backoffLimit` of 6 would re-run it six times, without `--resume` on a partially-completed pipeline (§6.2).
- **One trace covers the whole run.** Submit from the UI and confirm a single trace spans BFF → API → queue wait → Job pod → every stage → dbt. Two disjoint traces means `jobs.trace_context` is not propagated (§10.2). Then check the *final* stage and exit status are present — a missing tail is the `force_flush()` gotcha.
- **Logs carry no attribute values.** Grep a completed run's logs for a known name, email, and phone number from the seeded fixture. All three must be absent (§8.5).
- **The edge is actually internal.** From a network without Tailscale, `alice.dev.<domain>` must not connect; over the tailnet it must. And `kubectl get ingress -A` must list `er-web` only — er-api on an Ingress means the §4.1 boundary is fiction.
- **IMDS is unreachable from pods.** `kubectl exec` into an app pod and curl `169.254.169.254` with a one-second timeout — it must time out. hostNetwork system pods still reach it; that split is the design (§5.6).

**Phase 3 — refresh and spot resilience.** Run `make refresh-dev` and diff golden-record counts against prod. Then terminate a runner node mid-job and confirm the job requeues with `--resume` and completes — this validates the assumption §16 prices spot against. Separately, confirm a spill-heavy run is *not* evicted (§5.3).

**The existing test tiers remain the gate.** `make check` (fmt + clippy + entities-check + test) must pass unchanged, plus `make check-e2e` for the DuckLake + Postgres tiers and `make frontend` for the BFF. The Python-era contract tests named here previously (`test_dockerfile_contract.py`, `test_dbt_profiles.py`) went with the Python tree; the image contract is now asserted by the Dockerfiles' own build-time checks and by `make dev-up` rolling the built tag. The e2e tiers, not the fast one, are what catch cross-module regressions — run them before merging Dockerfile changes.

## 18. Open items

Measurements and decisions this design is still waiting on. (Resolved items — Auto Mode, the domain, the backend-design.md §7 class correction — have moved to [infrastructure-decisions.md](infrastructure-decisions.md).)

1. **Is the one-Postgres-instance constraint real?** §2 cites [backend-design.md](backend-design.md) §5 for the queue and the tenant advisory lock needing one cluster. PostgreSQL advisory locks are scoped per *database*, and the lock is taken on the tenant catalog DB while the queue lives in the control-plane DB — so they are in different databases regardless, and the serialization guarantee appears to come from the `jobs_one_active_per_org` unique index instead. If over-stated it removes a design option; §7.1 satisfies it either way, so this is not blocking.
2. **Where the thread-scaling curve flattens.** §6.5 makes vCPU the sizing lever on a 2→6 thread measurement that cut 10M-row runtime 40%. The measurement stops at 6, and is unlikely to stay linear: the same run showed **reconcile 14% *slower*** at 6 threads, a Python graph step that does not parallelize with DuckDB threads. A 10M run at 6 / 12 / 16 / 24 threads sets the top of the class ladder, and the same run with `ER_PROFILE_STORAGE` confirms spill at the 8 GB limit the L class actually uses.
3. ~~**Does EKS Auto Mode support the §5.1/§5.3 node controls?**~~ **Resolved 2026-10-10 — yes, Auto Mode chosen (D1).** Moved to [infrastructure-decisions.md](infrastructure-decisions.md) D-AUTOMODE / §5.5.
4. **Telemetry volume per pipeline run** — CloudTrail data events, and separately OTel log/span volume. Decides whether §7.3's scoped trail is affordable, whether §10.4's log levels are aggressive enough, and whether traces need tail sampling. Both are answerable from one instrumented 10M run, and both are §16 line items currently carrying a range instead of a number.
5. **Whether the dispatcher should watch pod logs or the runner should write its own progress** (§6.3). The recommendation is the latter, but it is a behavioural change to a path the frontend's job-detail view depends on, so it wants a decision before the Job launcher is written.
6. **DR posture.** Single region, no cross-region replication, no stated RPO/RTO for customer CRM data. Aurora PITR and S3 versioning cover deletion and corruption, not regional loss. Fine for now; it should be a stated accepted risk rather than an omission.
7. **DuckLake path storage** — absolute or relative in the catalog. Determines the size of `er lake rehome` (§13) and therefore whether Path A is permanent.
8. ~~**Hosted zone and domain.**~~ **Resolved 2026-10-10: `dupezero.com` at Cloudflare.** Moved to [infrastructure-decisions.md](infrastructure-decisions.md) (resolved open items); Cloudflare is the DNS plane, external-dns runs its Cloudflare provider with DNS-only records, ACM stays the certificate authority.
9. ~~**Resource classes need revising in [backend-design.md](backend-design.md) §7.**~~ **Resolved — `backend-design.md` §7 now specifies the vCPU/spill-tiered classes** (authority `server/src/erserver/sizing.py`). Rationale in [infrastructure-decisions.md](infrastructure-decisions.md) D-SIZING.
10. **API pod DuckDB sizing** — until the [backend-design.md](backend-design.md) §8 split is implemented, a tenant configured for a large runner also sizes the API pods' attaches (§2). The `app` NodePool is sized defensively as a result.
11. **What granularity Split Cost Allocation Data actually exposes.** §16.1 and §16.2 assume per-pod CUR rows carrying namespace and workload. Whether arbitrary pod *labels* surface as CUR columns decides whether per-tenant grouping is a direct query or needs joining through the `jobs` table on workload name. Either works; the second is more code. Worth confirming against one real CUR.
12. **Whether the internal cost model and the CUR reconcile.** §16.2 keeps two numbers — allocated at a blended rate in Postgres, billed in the CUR — and claims they should agree within ~10%. That tolerance is a guess until a month of both exists. If the gap is much wider the likely causes are spot price variance and pods that request far more than they use.
13. **Which Trust Services Criteria the first audit is scoped to.** Security is mandatory. **Confidentiality** is the natural second given the data, largely covered by §8.3 and §8.1. **Availability** is the expensive one, because it brings a documented RPO/RTO and *evidence of a restore test* — so §18.6's deferred DR posture becomes required work the moment Availability is in scope. Scoping to Security + Confidentiality first is the lighter path.
14. **The Anthropic data flow needs a documented position**, not just an inventory entry (§8.6). The assistant sends names, emails, and full golden records to a third party. Minimum: a DPA, a configured retention posture on the vendor side, customer disclosure, and a decision on whether tenants can opt out. Product work with a compliance deadline attached. **One of the four is now built**: the opt-out exists as a per-tenant toggle (`erweb.org_ai_settings`, V5), and a tenant that switches AI off makes no outbound call from either the assistant or enrichment. A tenant may also supply its **own** API key, in which case the data still leaves but under that tenant's own vendor agreement rather than ours — which is a different disclosure, not an absent one. The DPA, the retention posture and the disclosure text remain open, and they are the three that need a lawyer rather than a commit.
15. **Does `sslmode=verify-full` work with the Aurora CA bundle as the images are built?** Partially answered by inspection: the Dockerfile installs no `ca-certificates` or RDS bundle, so the runtime has only what `python:3.12-slim` ships. Expect an image change (install `ca-certificates` plus the RDS global bundle), not a secret change; what stays open is verifying the bundle path wiring in each DSN.
16. **Cold-node image pull time** for the current `er-pipeline`. Decides how far down the §11.1 ladder to climb — SOCI alone, the image split, or snapshot prebake — and nothing measures it today.
17. ~~**Whether Phase 4's two credential retirements land as one change or two.**~~ **Resolved, and it turned out to be one.** The SES half (§9.3) landed with the mailer, because writing the Rust sender against the SES API was cheaper than building the SMTP path the retirement was scheduled to undo. No SES key was ever created, so "no static keys anywhere" now rests on §9.2's S3 credential chain alone.
