# Infrastructure design: AWS and Terraform for the ER platform

*A design for the AWS footprint that runs the platform specified in [backend-design.md](backend-design.md): two accounts, one EKS cluster per environment, a namespace-per-developer dev environment, and a security model where prod is readable by the team but writable only from an explicitly assumed role. This is a design document — no Terraform is written yet. Written 2026-10-02; last revised 2026-10-03 (third pass).*

> **Implementation status (2026-10-03).** Nothing in this document is built. The
> repository contains no Terraform, no Kubernetes manifests, no Helm charts, and
> no container registry wiring; CI builds `er-pipeline:ci` with `load: true` and
> pushes it nowhere. [backend-design.md](backend-design.md) §4–§9 already
> specifies the *application* topology this design serves, and `server/README.md`
> records the k8s Job launcher as deliberately unbuilt behind an existing
> `launch` seam. This document covers the cloud substrate only; it does not
> redesign the application.

> **Revision note (2026-10-03).** Two rounds of changes. First, the original
> draft asserted three cost savings it had not substantiated — Karpenter "scale
> to zero", Aurora auto-pause, and interface endpoints as a NAT-avoidance
> measure. All three were wrong or incomplete and the dev estimate was optimistic
> by roughly 2×; §5.2, §7.1, and §16 are rewritten. Second, §6 (job execution)
> and §10 (observability) are new, and §6.5 revises run sizing from
> memory-tiered to CPU-tiered on the strength of measurements already in the
> repo — which also changed the runner instance family from `r6id` to `c6id`.
> §18 carries what this design is still waiting on.

> **Revision note (2026-10-03, third pass).** A cross-examination against the
> repository and current AWS behaviour. What changed: the §6.2 Job spec omitted
> the EFS mount the runner cannot run without — and the dispatcher turns out to
> read org configs off that filesystem on every idle tick, so it mounts it too;
> dev environments holding real records were implicitly internet-facing behind
> nothing but the application login, and §4.1 now puts them behind an internal
> ALB reached over Tailscale; email was treated as a future feature when it is
> a built, wired subsystem (§7.5), and SES SMTP structurally cannot use
> temporary credentials, which adds a second static key to §8.1's ledger until
> Phase 4 (§9.3); §4's "let the node cache do the work" was defeated by the
> runner pools' own scale-to-zero (§11.1); single-instance-type spot pools were
> the worst shape to buy spot in (§5); §14.2 now pins who may run Terraform and
> deploy, because the §8.2 SCPs exempt a role this document had never named;
> and §18.3's Auto Mode question is answered — NVMe RAID0 is automatic — which
> reopens §5.5. New sections: §4.1, §5.6, §7.5, §9.3, §11.1, §14.2.

---

## 1. Context and goals

The platform today runs as host processes against a Docker Compose substrate: `uvicorn` for the API, `python -m erserver.dispatcher` for the dispatcher, `next dev` for the UI, with Postgres and MinIO in containers ([frontend/dev/compose.yaml](../frontend/dev/compose.yaml), `make run`). That is a correct development stack and a dead end for anything else — there is no deployable environment, no registry, and no notion of dev versus prod anywhere in the repository.

This design adds that substrate, with five goals:

- **Run the four deployables on EKS.** `er-api`, `er-dispatcher`, `er-runner` ([backend-design.md](backend-design.md) §4) plus the Next.js BFF ([frontend-design.md](frontend-design.md) §4.3).
- **Give every developer a complete, isolated environment** in their own namespace, with its own S3 lake prefix, DuckLake catalog databases, and transactional control-plane database — created, suspended, and destroyed in seconds.
- **Hold real production data in lower environments**, protected by IAM, encryption, and audit rather than by scrubbing or synthesis.
- **Make prod readable by the whole team and writable only from an explicitly assumed role**, enforced structurally rather than by policy correctness.
- **Resolve every secret from AWS Secrets Manager** through the `secret://` indirection that already exists in [server/src/erserver/secrets.py](../server/src/erserver/secrets.py).

**Decisions taken:**

| Decision | Choice | Why |
|---|---|---|
| Accounts | **One** (rev 4, operator decision 2026-10-10) — environments separated by `er-dev-*`/`er-prod-*` prefix and by permission-set policy, not account boundary | Minimal operational surface. The trade: SCPs evaluate against nothing, so every §8.2 guardrail is IAM-layer — §3 records what that gives up |
| Identity | IAM Identity Center with permission sets | Short-lived credentials; no long-lived human access keys to leak or rotate |
| Clusters | One EKS cluster per environment, **dev first — prod deferred until there is prod traffic** | Cluster-scoped failures and prod secrets both stay inside one environment (§5.4). Deferring prod costs nothing today and keeps the decision for when it matters |
| Postgres | **In-namespace StatefulSet in dev, Aurora in prod** | The catalog is the lake's metadata and must persist, but it does not need a managed server standing up for one developer. ~$3/mo of EBS against Aurora's ~$44/mo floor (§7.1) |
| Idle cost | **Suspend the namespace**, not per-resource autoscaling | One control drives node count, database, and compute to zero together. The first draft listed these as three independent savings; they are one (§5.2) |
| Pipeline runs | **A native Kubernetes Job per run**, behind the `launch` seam that already exists — not Argo Workflows | The DAG is inside the process, not across pods. Argo would duplicate a complete retry/state machine and add a second scheduler that cannot honour the one-active-job-per-org index (§6.1) |
| Run sizing | **Tiered on vCPU with a near-constant memory floor**, sized per run from the record count | 10M records peaked at 9.37 GiB, and 2→6 threads cut runtime 40% with memory flat. This inverts [backend-design.md](backend-design.md) §7's memory-tiered classes (§6.5) |
| Telemetry | **OpenTelemetry to a collector**, exporter swappable; CloudWatch + X-Ray first | Three runtimes, one standard; a run crosses five process boundaries; and the self-hosted roadmap item makes vendor neutrality a product requirement, not a preference (§10) |
| Cost attribution | **Tags + CUR + EKS split cost allocation**, plus a per-run cost figure in Postgres | Pods are not AWS resources, so tags alone cannot see them. Per-developer falls out of namespace-per-developer, per-tenant falls out of pod-per-run (§16.1) |
| SOC 2 | **Build only what cannot be retrofitted**: 400-day audit trails, a compliance-mode Object-Locked archive, CloudTrail log validation, branch protection | Type 2 fails on evidence, not architecture. Policies and assessments can wait; six months of logs cannot be produced after the fact (§8.6) |
| Region | `us-east-1`, single region, no DR replication in this phase | Already the only region in the repo (`ER_S3_REGION` in [frontend/dev/env.sh](../frontend/dev/env.sh), [configs/default.yaml](../configs/default.yaml)) |
| Edge access | **Dev behind an internal ALB reached over Tailscale; prod internet-facing behind WAF.** `er-api` is never on any load balancer | Dev namespaces hold real records, and an application login as the only boundary is the posture §8.2 rejects. The BFF is the sole browser surface by construction (§4.1) |
| Email | **SES** — SMTP credential now, SES API + IRSA in Phase 4 | The mailer is built and wired (invites, password reset, job notifications); SES SMTP cannot use temporary credentials, so the interim is a second scoped IAM user (§7.5, §9.3) |
| Deploys | **Terraform and Helm apply from CI on `main`; the prod-scoped Terraform role trusts only CI and `ProdAdmin`** | The §8.2 guardrail denies exempt Terraform, which makes that role's trust policy part of the guardrail rather than plumbing (§14.2) |
| Product SSO | **Direct OIDC in the BFF — Salesforce, Google, Microsoft — via `openid-client`; no broker; existing DB sessions kept** | Zero new subprocessors and zero MAU pricing; the session layer already exists and survives unchanged. WorkOS is the named path when enterprise SAML arrives ([frontend-design.md](frontend-design.md) §2.4) |

**Deliberately deferred:** the prod cluster and prod Aurora (no prod traffic yet — §15); the staging environment (no users yet); `arm64`/Graviton images (~20% cheaper on the app tier, but CI runs on x86 and multi-arch buildx via QEMU hurts the inner loop); multi-region anything, and with it any real DR posture (§18.6).

**Considered and not chosen:** Argo Workflows for the job path (§6.1), a single cluster for all environments (§5.4), Aurora Serverless v2 auto-pause in dev (§7.1), VPC interface endpoints (§4), memory-tiered resource classes (§6.5), ALB OIDC authentication against Identity Center (§4.1). EKS Auto Mode moves off this list to *reopened* — its blocker dissolved under research (§5.5).

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

**Revision 4 (2026-10-10): one account.** Revisions 1–3 specified a three-account
organization (`mgmt`/`nonprod`/`prod`). The operator chose to collapse to a single
account to keep the operational surface minimal — one login, one bill, one root
user, no cross-account plumbing. This section records what that trades away and
what stands in for it; the three-account design remains in git history as the
shape to return to when the team grows.

```
Organization root (kept — Identity Center requires it; otherwise inert)
└── er (797781631727)       everything: Identity Center, billing, the
                            Object-Locked audit archive (§8.6), ECR, TF state,
                            dev environments, and — behind the §15 prod gate —
                            prod, separated by prefix and role, not account
```

Environment separation is by **name and by role**: resources carry an
`er-dev-*` / `er-prod-*` prefix (buckets, roles, clusters, secrets), and the
Identity Center permission sets scope who may touch which prefix (§8.1, §8.2).

**What the collapse gives up — recorded deliberately, accepted by the operator:**

- **SCPs no longer apply to anything.** Service control policies never evaluate
  against the organization's management account, which is now the only account.
  Every §8.2 guardrail is therefore an IAM-layer deny carried by the permission
  sets — enforceable, but one policy edit from absent, where the OU boundary was
  structural. This was the prior revision's central argument for three accounts.
- **Blast radius is shared.** Dev mutation is unrestricted by design (§8.1), and
  dev mistakes now share an account — and its service quotas — with prod data.
- **The audit archive lives with the workloads.** The compensating control is S3
  Object Lock in compliance mode: 400-day retention that not even the root user
  can shorten, preserving tamper evidence without an account boundary (§8.6).
- **SOC 2 CC6 is evidenced from policy text** rather than account membership.

The former `nonprod` member account (660360495170) is parked empty at the
organization root at zero cost; close it or re-adopt it if the split returns.

## 4. Network, per environment

One VPC per environment, `/16`, three availability zones, public and private subnets.

- **An S3 Gateway Endpoint is mandatory, not an optimization.** A large run moves hundreds of gigabytes between the runner and the lake; routing that through a NAT Gateway at $0.045/GB would dominate the bill. The gateway endpoint is free and keeps the traffic on the AWS backbone.
- **One NAT Gateway in nonprod**, three in prod for AZ independence once prod carries traffic. The single nonprod NAT means cross-AZ traffic from the other two subnets at $0.01/GB each way — real but small next to a per-AZ NAT.

**Interface endpoints are deliberately not used.** The first draft specified them for ECR (api + dkr), Secrets Manager, STS, and CloudWatch Logs, justified as keeping node bootstrap off the NAT path. That reasoning ignored their price: five endpoints × three AZs × ~$0.01/hr is **~$110/mo per environment**, against the $33 NAT they were avoiding. For a one-developer footprint this inverts the decision. Non-S3 traffic goes through the NAT, and the only volume that looked material was ECR image pulls — but it is not: ECR serves image *layer blobs* from S3 (the regional `prod-us-east-1-starport-layer-bucket`), so the bytes of a pull ride the free gateway endpoint and only the manifest and auth calls cross the NAT. What a cold node's pull costs is time, not money — §11.1.

Revisit this when there are enough nodes that per-GB NAT processing exceeds the fixed endpoint cost, or when a compliance requirement forbids the NAT path.

### 4.1 Edge access: who can reach these environments at all

The earlier revisions specified Ingresses and an ALB and never said *from where* — which, unexamined, meant internet-facing. For dev that is a finding, not an oversight to forgive: §13 deliberately fills developer namespaces with real production records, and an internet-facing ALB leaves the Next.js session check as the only thing between those records and the internet. §8.2 spends an entire account boundary refusing exactly this posture — one policy away from exposure — at the IAM layer; the edge deserves the same reasoning.

**Dev: an internal ALB, reached over Tailscale.** A subnet router on a `t4g.nano` advertises the VPC CIDR and resolves the private zone via the VPC resolver — roughly $4/mo, and Tailscale's free tier covers three users. The managed alternatives price themselves out at this scale: Client VPN has a ~$73/mo idle floor before connection hours; Verified Access runs ~$197/mo per application. ALB OIDC authentication was considered and does not work here: Identity Center cannot act as ALB's OIDC provider — it exposes no userinfo endpoint — so that path requires a Cognito user pool federated to Identity Center over SAML, which is real plumbing to approximate what the tailnet gives structurally.

**Prod, when it exists: internet-facing, behind WAF.** Customers arrive from the internet, so the boundary moves from network to WAF plus application auth. The non-negotiable rules are rate-based: the frontend exposes five unauthenticated paths — six once SSO lands, with [frontend-design.md](frontend-design.md) §2.4 adding `/api/auth/sso` — and three of them — `/api/auth/login`, `/api/auth/password-reset` (which also sends email), and `/api/invites` — accept unauthenticated writes with no in-app rate limiting anywhere today; the SSO start and callback routes join the rate-ruled set the day they exist. WAF rate rules scope to URI paths with thresholds down to 10 requests per window, at roughly $8/mo all-in.

**`er-api` is on no load balancer in any environment.** Already true by construction, and it needs only to be kept true: the frontend defines zero `NEXT_PUBLIC_*` variables by stated policy, every browser call is same-origin to the BFF, and `ERSERVER_BASE_URL` lives in server-only modules. Only `er-web` gets an Ingress; erserver stays a ClusterIP service. One loose end to close while here: FastAPI serves `/docs`, `/redoc`, and `/openapi.json` unauthenticated by default and nothing disables them (§14.1) — harmless in-cluster, pointless to leave on.

## 5. Compute: EKS

EKS per environment, on the newest minor current at build time — 1.36 as of this revision; the 1.34 previously written here was already two behind, which is why the version is pinned in Terraform rather than in this document. API-based access entries rather than the legacy `aws-auth` ConfigMap, and these addons: EBS CSI, EFS CSI, AWS Load Balancer Controller, external-dns, External Secrets Operator, Karpenter.

A small managed node group (2 × `t4g.medium`, on-demand) hosts Karpenter, CoreDNS, and the controllers — Karpenter cannot provision the nodes it runs on.

| NodePool | Instance types | Capacity type | Serves |
|---|---|---|---|
| `app` | `m6i.large`, `m6a.large` | spot in nonprod, on-demand in prod | `er-api`, `er-web`, `er-dispatcher` |
| `runner-sm` | `c6id.2xlarge`, `c8id.2xlarge`, `c5ad.2xlarge`, `c5d.2xlarge`, `m6id.2xlarge` — 8 vCPU, local NVMe | spot, on-demand fallback | S and M classes |
| `runner-l` | `c6id.4xlarge`, `c8id.4xlarge`, `c5ad.4xlarge`, `m6id.4xlarge` — 16 vCPU, local NVMe | spot, on-demand fallback | L class |

**Compute-optimized with local NVMe (`c6id`), not memory-optimized (`r6id`).** The first draft chose `r6id.4xlarge` to host a 64 GiB pod, following [backend-design.md](backend-design.md) §7's resource classes. The measurements say that was wrong on both counts: a 10M-record reload peaked at **9.37 GiB** of container memory, and going from 2 to 6 DuckDB threads cut its runtime **40%** with memory essentially flat. Runtime is CPU-bound and memory is nearly constant in corpus size, because DuckDB spills rather than growing its working set — see §6.5 for the full evidence and the revised classes.

So the right node has many vCPU, modest RAM, and a large NVMe for spill. `c6id.4xlarge` matches `r6id.4xlarge`'s 16 vCPU and 950 GB of NVMe while dropping 128 GiB of RAM to 32 GiB, for roughly **a third less per hour**. `c6id.8xlarge` (32 vCPU) is the top of the ladder, bounded by the `le=32` cap on `duckdb_threads` in the existing resources endpoint.

**Many families and both capacity types, deliberately.** The second revision listed exactly one instance type per runner pool, which is the worst shape to buy spot in — one type in one size raises both the interruption rate and the odds of plain unavailability. Karpenter makes width free: list `spot` and `on-demand` together in one NodePool and it strictly prefers spot, falling back to on-demand automatically when spot capacity comes up empty — the documented fallback pattern, not a cost risk. The families are every current x86 compute-shaped instance with local NVMe: `c6id`, `c8id` (GA February 2026, same 474/950 GB NVMe shapes), `c5ad`, `c5d` — there is no `c7id`; Intel's seventh x86 generation skipped the NVMe variant — plus `m6id` as a deeper fallback that merely wastes some RAM. One exclusion is load-bearing: **`c5d.4xlarge` carries only 400 GB of NVMe against the L class's 400 GiB spill limit (§5.3)** — negative headroom before the image and kubelet overhead land — so it stays out of `runner-l`, while `c5d.2xlarge` (200 GB against S/M's 40–150 GiB) stays in `runner-sm`.

Spare capacity on these nodes is not waste: the one-active-job-per-org constraint means two *different* tenants' runs bin-pack onto one node, and at ~14 GiB per L pod a `c6id.4xlarge` holds two.

### 5.1 Karpenter disruption settings

The first draft said "all NodePools scale to zero" and specified no disruption settings at all. The defaults are actively wrong for this workload, and the wrong choice here is invisible — it shows up as jobs taking longer, not as errors.

| NodePool | `consolidationPolicy` | `consolidateAfter` | Pod annotation |
|---|---|---|---|
| `app` | `WhenEmptyOrUnderutilized` | `1m` | — |
| `runner-sm`, `runner-l` | **`WhenEmpty`** | `30s` | **`karpenter.sh/do-not-disrupt: "true"`** |

**The runner pools must not use `WhenEmptyOrUnderutilized`.** That is the setting reached for to save money, and it permits Karpenter to drain a busy node to bin-pack onto a cheaper one. On a 10M-row run that discards up to an hour of work. The retry policy in §2 makes it *survivable* — exit code `None`, requeue with `--resume` — which is precisely why it would register as unexplained slowness rather than as an incident.

`karpenter.sh/do-not-disrupt` on the runner pods blocks voluntary disruption for the life of the job. Nodes still empty ~30s after the pod exits. `expireAfter` on the NodePool template (72h) handles AMI hygiene, since a do-not-disrupt pod otherwise pins a node indefinitely.

Spot interruption is *involuntary* and cannot be annotated away. Karpenter drains on the two-minute notice, `--resume` picks up at the last completed stage, and that is the trade accepted in exchange for spot pricing.

### 5.2 Why idle cost is a namespace-suspend problem, not an autoscaling one

Karpenter removes a node when its pods are gone. It cannot remove a node whose pods are still scheduled. A developer namespace running `er-api`, `er-dispatcher`, and `er-web` around the clock keeps an `app` node alive permanently, and **no Karpenter setting changes that.** The first draft's "scale to zero" claim was about the runner pools only, and read as though it covered the environment.

The control that actually works is suspending the namespace:

```
make dev-suspend DEV=alice     # scale all Deployments + the Postgres StatefulSet to 0
make dev-resume  DEV=alice     # scale them back; PVC and S3 prefix untouched
```

This cascades. With the pods gone the `app` node empties and Karpenter reclaims it; the dispatcher's permanent leader connection goes away, so nothing holds the database open; and the Postgres pod stops billing compute while its EBS volume persists. One action, the whole environment to near-zero — against three separately-tuned mechanisms that each only moved part of the bill.

A `CronJob` suspends idle namespaces, and the predicate is worth writing down because the obvious signal does not exist — there is no request log anywhere in the system. What exists is `erweb.sessions.last_seen_at`, slid on activity but throttled to at most hourly ([frontend/src/lib/auth/session.ts:13](../frontend/src/lib/auth/session.ts)): coarse, and sufficient for a four-hour threshold. Suspend when `max(last_seen_at)` is older than four hours **and** no job is queued or running — the second clause is what keeps the CronJob from burning a `--resume` attempt under a live run. For a 1–2 person team that is most of the day. Resume is a single `helm upgrade` and takes under a minute.

### 5.3 Spill, ephemeral storage, and the eviction trap

DuckDB writes temp files to a `.tmp` directory relative to the process working directory, and nothing in the engine emits `SET temp_directory`. The two concrete paths are `/app/.tmp` for the CLI and runner and `/app/dbt/.tmp` for the dbt subprocess ([benchmarks/storage_sampler.py:29](../benchmarks/storage_sampler.py)).

Both runner NodePools set `instanceStorePolicy: RAID0` on their EC2NodeClass, striping the local NVMe and making it the kubelet's ephemeral-storage root. An `emptyDir` then lands on NVMe automatically, so mounting `emptyDir` at those two paths satisfies the spill requirement with no engine change.

**The first draft stopped there, and that was incomplete.** An `emptyDir` counts against the pod's ephemeral-storage accounting, so the sizing has to be explicit in two places or the design has a trap in it:

- **With no limit**, one runaway run fills the node's NVMe and the kubelet evicts *every* pod on it under disk pressure, including other tenants' runners.
- **With a limit set too low**, the runner itself is evicted partway through. Eviction yields exit code `None`, which requeues with `--resume` — and then evicts again at the same point. `max_attempts` (default 3) bounds this, so it is not an infinite loop, but it is three discarded runs and a job that fails for a reason the logs attribute to the OS rather than to a misconfigured volume.

So the runner pods carry an explicit `requests.ephemeral-storage` — which is also what lets the scheduler avoid stacking two spill-heavy pods onto one node — and an `emptyDir.sizeLimit` generous enough for the real figure plus headroom, but below the node's capacity. On a 950 GB `c6id.4xlarge` that is roughly 400 GiB per L runner, which fits two.

Crucially, the 141 GiB figure is measured at a **4 GB** DuckDB memory limit, and §6.5 keeps the L class at 8 GB rather than the 64 GiB the original resource classes called for — so the measurement was taken near the configuration this design actually runs, and transfers. Had the design kept a 64 GiB DuckDB limit, this number would have been an extrapolation across a 16× change in buffer size and worth nothing. Spilling heavily to free instance-store NVMe instead of buying RAM to avoid it is the deliberate trade (§6.5).

### 5.4 One cluster per environment, and why not one cluster total

Collapsing dev and prod into one cluster with namespace separation saves roughly **$97/mo** — a $73 control plane and a second system node group — plus another $16 if prod gets its own ALB.

What the second cluster buys:

- **Cluster-scoped blast radius.** A bad CRD, a webhook that fails closed, a CNI upgrade, a Karpenter version bump: all cluster-wide, none able to cross.
- **A staging-shaped upgrade path without a staging environment.** Dev goes to EKS 1.35 first and reveals what breaks. With one cluster, dev and prod upgrade in the same action — a genuinely bad property for a two-person team with no staging.
- **Secret separation, which is the decisive argument.** In one cluster, prod's `ERSERVER_OPERATOR_TOKEN` — a cross-tenant operator credential — and `ERWEB_CREDENTIAL_KEY`, which unseals every org's stored API keys, would sit in the same etcd as namespaces where developers hold admin. Namespace RBAC separates them, but any cluster-scoped escape crosses into prod, and dev holds real data so the incentive exists.

That last point is also a consistency argument. §8.2 justifies two accounts on the grounds that structural separation beats policy correctness. A single cluster adopts precisely the posture that reasoning rejects; if it were acceptable at the Kubernetes layer, a single AWS account should have been acceptable too.

**The resolution is to keep two clusters but build only one now.** There is no prod traffic. Deferring the prod cluster costs nothing today and preserves the boundary for when it carries something. §15 sequences it accordingly.

### 5.5 EKS Auto Mode — reopened: the blocker dissolved

Auto Mode manages Karpenter and the core addons, removing both the system node group and the operational burden of upgrading them. For a two-person team that trade — roughly a 12% EC2 surcharge against not operating Karpenter — is plausibly worth taking.

The second revision declined it on one question: whether §5.3's `instanceStorePolicy: RAID0` is expressible. **The answer is yes — and better than expressible, automatic.** Auto Mode formats instance-store NVMe for ephemeral use on its own, striping RAID0 across multiple drives, with no field to set; constrain the NodePool to NVMe families and `emptyDir` lands on it. Auto Mode also enforces IMDSv2 with hop limit 1 unconditionally (§5.6) and ships SOCI parallel pull on by default (§11.1) — two things this design wants anyway.

What it would still cost: the management fee; no custom AMIs or `blockDeviceMappings`, so §11.1's snapshot-prebake rung is off the table; and since April 2026 Auto Mode's managed instances are hidden from `DescribeInstances`, which confuses inventory tooling. One check remains before flipping: whether Auto Mode's NodePool surface honours the §5.1 disruption controls — `do-not-disrupt` and a `WhenEmpty` equivalent — because those are the safety of the runner path, and losing them silently would reproduce exactly the invisible-slowness failure §5.1 exists to prevent. §18.3 now carries that residue, not the NVMe question. If it passes, Auto Mode is the better fit for this team and the system node group line simply disappears.

### 5.6 Node and pod hardening

None of this appeared in the first two revisions, which is itself the finding — the repository and this document had zero occurrences of IMDS, `securityContext`, or Pod Security between them.

- **IMDSv2 required, hop limit 1, on every node** — the managed node group and both EC2NodeClasses. This is what stops a pod — including a runner processing tenant data — from harvesting the node role off the metadata service. It breaks nothing this design runs: kubelet, the VPC CNI, and the CSI drivers are hostNetwork and keep their access; IRSA never touches IMDS; Phase 1's static S3 keys arrive by env var. It is the EKS best-practices recommendation verbatim, and Auto Mode enforces it without asking (§5.5).
- **Pod Security Standards labels on every namespace** — `baseline` enforced from day one, `restricted` the target. The gap to `restricted` is that no image declares a `USER` today — the engine runs as root — which is an application change, not a cluster setting (§14.1).
- **Cluster API-data encryption: the default is now enough.** EKS has envelope-encrypted all Kubernetes API data with an AWS-owned key on every 1.28+ cluster since March 2025. A customer-managed key is deliberately *not* associated: association is irreversible, and a disabled or deleted CMK degrades the cluster unrecoverably — risk with no §8.3-style cross-account payoff attached. Same reasoning as the ECR exception in §8.3.

Account-level counterparts — S3 Block Public Access in all three accounts, MFA on every root user, no root access keys — are one-liners that live in Phase 0 (§15).

## 6. Job execution: the runner as a Kubernetes Job

A pipeline run is one ephemeral pod, scoped to one tenant, that exits when the run finishes. That is not a new idea here — it is what [backend-design.md](backend-design.md) §4 already specifies, and `server/README.md` records the k8s Job launcher as deliberately unbuilt behind an existing seam:

```python
dispatcher.launch_runner(payload, env, on_stage=, should_cancel=)
```

Today that seam calls `subprocess.Popen([sys.executable, "-m", "erserver.runner", ...])` with the tenant's `ER_*` env merged in. Substituting a Kubernetes Job is a change to one function, which is the whole reason the seam exists.

The rationale for pod-per-run rather than long-lived workers is worth restating because it explains the shape of everything below. The tenant seam is process-scoped env ([src/er/lake/env.py](../src/er/lake/env.py) is the sole reader of `ER_*`); the Postgres advisory writer lock is session-scoped, so a dying pod releases it with no cleanup path needed; and memory bin-packing against an 8–64 GiB per-run envelope only works if the allocation is per-run.

### 6.1 Argo Workflows — considered, not chosen

There is real prior art for multi-tenant ephemeral data pipelines on Argo Workflows, and the instinct is sound. It is the wrong fit here for a specific reason: **the DAG is inside the process, not across pods.**

`run_all_full` executes ingest → standardize → train → match → reconcile → assemble as library calls in a single process via [src/er/service.py](../src/er/service.py), with dbt invoked as a subprocess inside that same pod. There is no artifact to pass between pods, no step to fan out, and no dependency graph for Kubernetes to resolve. Argo's core value — orchestrating multi-step DAGs across pods with artifact passing — would have nothing to orchestrate.

What it would add is a **second, competing copy of control logic that already exists and is tested**:

- **Retry semantics.** [server/src/erserver/policy.py](../server/src/erserver/policy.py) keys retries to the engine's exit-code taxonomy: `0` succeeded, `10` no-op, `2` fails permanently (config error — must never retry), `3` + `lock_conflict` requeues after 30s *without consuming an attempt*, `1` + `transient_io` or a `None` exit requeues *with `--resume`* under exponential backoff. Argo's `retryStrategy` does not know any of that, and expressing it there would put the policy in two places.
- **Two sources of truth for run state.** The `jobs` table is what the API's `/runs` endpoints and the frontend's job detail view read, with live per-stage progress in `jobs.progress`. An Argo workflow object would shadow it, and the two would disagree the first time one of them was restarted.
- **Two schedulers.** Cron already lives in the dispatcher's leader loop via `croniter` ([schedules.py](../server/src/erserver/schedules.py)). Argo CronWorkflows would be a second thing able to start a tenant's run, and it *cannot* honour the `jobs_one_active_per_org` partial unique index, because that guarantee is enforced by the dispatcher's `FOR UPDATE SKIP LOCKED` claim query. Two dispatchers means the single-writer-per-tenant property is gone.

[backend-design.md](backend-design.md) §5 already rejected Celery and Redis on structurally identical grounds — "at-least-once redelivery is actively wrong for an engine that deliberately never auto-retries." Argo is a different tool with the same mismatch: an orchestrator with its own opinions about retries and state, layered over an engine whose state machine is already complete and deliberate.

There is also a roadmap cost. [backend-design.md](backend-design.md) §15 phase 5 contemplates a self-hosted option, and the existing non-k8s fallback (the dispatcher forking a subprocess) serves that today. Argo would become a hard dependency for self-hosters.

**When to revisit:** if a run ever becomes a genuine multi-pod DAG — sharding match across pods, which the one-writer-per-tenant constraint currently forbids — or if workflow visualisation for non-engineers becomes a product requirement. Neither is true now.

Argo **CD** is a separate question. GitOps for deploying the platform itself is defensible and does not touch the job path; it is simply out of scope here.

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
    EFS access point → ERSERVER_CONFIG_ROOT, ERSERVER_DROP_ROOT   ← §7.4 — omitted by the
                                            second revision; the Job cannot run without it
  envFrom: per-run Secret (the tenant's ER_* overlay)
```

**`backoffLimit: 0` is the single most important field.** The default of 6 would have Kubernetes retry a failed run on its own schedule, with no knowledge of the exit-code taxonomy — re-running a config error six times, and worse, re-running a partially-completed pipeline *without* `--resume`.

**The EFS mount is the second most important line, because its absence fails late.** The runner's first act is opening the config file by path ([server/src/erserver/runner.py:31](../server/src/erserver/runner.py)), and an incremental import lists its CSV drops from `drop_root` at ingest time ([src/er/ingest/sources.py:146](../src/er/ingest/sources.py)). Half an escape hatch exists and is worth recording: config *content* could travel inline — `configsvc.validate_yaml` already round-trips inline YAML through a temp file, so the runner could do the same with no engine change — but drops cannot leave the filesystem, because `storage.drop_dir` must be an absolute local path ([src/er/config/schema.py:285](../src/er/config/schema.py)) and discovery is a literal directory listing. Inline config is a later optimization, not a way out of the mount. The dispatcher needs the same mount for its own reasons (§2): `drain_staged` re-loads an org's config on every idle pass.

Cancellation keeps today's semantics: the dispatcher deletes the Job, Kubernetes sends `SIGTERM`, and the engine stops at the next stage boundary. `terminationGracePeriodSeconds` must exceed the engine's stage-boundary check interval and the dispatcher's existing `TERMINATE_GRACE_SECONDS = 10.0`.

The dispatcher's ServiceAccount gets a namespace-scoped **Role**, not a ClusterRole: `create`/`get`/`list`/`watch`/`delete` on `batch/jobs`, `get`/`list`/`watch` on `pods`. Namespace-scoped matters — a dispatcher able to create pods cluster-wide would undercut §5.4's argument for separate clusters.

### 6.3 Progress reporting is the one thing this breaks

Today `launch_runner` reads the subprocess's **stderr pipe** directly, parsing one JSON record per stage and writing it into `jobs.progress` while the run is live. A Job pod has no pipe to the dispatcher, so this is the one place where substituting a Job is not transparent, and it needs deciding before the launcher is written.

Two options:

1. **Dispatcher streams pod logs** via `read_namespaced_pod_log(follow=True)`. Preserves the existing parsing code, but holds one long-lived HTTP stream per concurrent run, loses progress if the dispatcher restarts mid-run, and is vulnerable to log rotation on a multi-hour job.
2. **The runner writes its own progress** to the control-plane database. The runner already holds a DSN and already emits exactly these records; this makes the write direct instead of relayed.

**Option 2 is the recommendation.** It survives dispatcher restarts, removes N long-lived streams, and makes the dispatcher a pure scheduler rather than also a log relay. It is a small change to [server/src/erserver/runner.py](../server/src/erserver/runner.py) and it is on the same seam that §10 needs for trace context — so the two should be designed together rather than sequentially.

### 6.4 "Contained to that tenant" — what the containment actually is

Worth being precise, because the Kubernetes-shaped intuition does not match the tenancy model. [backend-design.md](backend-design.md) §7 makes a tenant a **database, a schema, and an S3 prefix** — not a namespace. In prod every tenant's runs execute as Jobs in one platform namespace; the per-namespace split in §12 is per *developer*, not per tenant.

A run's isolation therefore comes from four things, none of them a namespace boundary:

- **Process-scoped env.** `ER_CATALOG_DSN`, `ER_LAKE_DATA_PATH`, `ER_LAKE_METADATA_SCHEMA` injected per run; the pod can address exactly one tenant's catalog and prefix for its whole life.
- **Ephemerality.** The pod exits with the run, so there is no long-lived process that has ever held two tenants' credentials.
- **A dedicated catalog database** per tenant, so no tenant's tables share a database with another's.
- **The advisory writer lock**, which the pod holds for its session and releases by dying.

Namespace-per-tenant would add hard RBAC, NetworkPolicy, and quota boundaries, at the cost of namespace proliferation across hundreds of tenants, per-namespace secret plumbing, and a dispatcher needing cluster-wide job-create rights — which trades away more isolation than it buys. It is the right shape for the enterprise tier that [backend-design.md](backend-design.md) §7 already earmarks for dedicated buckets and STS-scoped credentials, and not before.

One forward-looking note: once §9.2's IRSA work lands, the per-run pod can carry a **per-tenant ServiceAccount** whose role is scoped to that tenant's prefix alone. That is the "per-tenant STS credentials" item in [backend-design.md](backend-design.md) §15 phase 4, and it is the point at which a compromised runner stops being able to address any other tenant's data. The Job spec should anticipate a ServiceAccount parameter even while every run shares one.

### 6.5 Sizing the Job: CPU is the lever, memory is nearly flat

Because the dispatcher knows the tenant's record count before it launches anything, the Job can be sized per run rather than drawn from a fixed class. More importantly, **the measurements say the thing to scale is CPU, not memory** — and that inverts the resource classes this design started from.

The evidence, all from runs already in the repo:

| Measurement | Source | What it shows |
|---|---|---|
| 10M-record full reload: 2 vCPU, 2 DuckDB threads, **4 GB** DuckDB limit, 10 GiB container. Peak container memory **9.37 GiB** | [performance.md](performance.md) | Ten million records completed in a 10 GiB container |
| Same corpus, 2 → 6 threads at the **same 4 GB** limit: 6,458s → 3,857s (**−40%**), peak memory 9.14 GiB. Work in progress, one pass per arm; the 2-thread reference is the Splink-5 campaign's 10M run, not the 1h 46m trial above | 2-thread arm: [measurements/splink5-20260922.json](measurements/splink5-20260922.json); 6-thread arm: [performance.md](performance.md), per-arm detail in git history | Threads buy time. Memory stayed flat while runtime fell 40% |
| 1M, 2 threads/4 GB → 6 threads/6 GB: 96.2s → 75.1s (−22%), peak 4.06 → 4.26 GiB. Work in progress; the DuckDB limit changed with the threads, so not a pure-thread comparison | [performance.md](performance.md), per-arm detail in git history | Same shape an order of magnitude down |

The reason memory stays flat is that DuckDB **spills**, and spill is why §5.3 exists. A larger corpus does not need proportionally more RAM; it needs somewhere to put 141 GiB of temporary files. Local NVMe comes free with the instance while RAM is billed, so the economically correct posture is to *embrace* spilling to instance store rather than buying memory to avoid it.

This means [backend-design.md](backend-design.md) §7's resource classes — S 8 GiB, M 24 GiB, **L 64 GiB** — are over-provisioned by roughly 6× on the dimension that does not matter, and silent on the dimension that does. Ten million records ran in 10 GiB. A 64 GiB pod would spend most of its allocation idle while still taking 1h04m, because nothing in that figure was memory-bound.

**Revised class shape — tiered on vCPU, with a near-constant memory floor:**

| Class | Records | vCPU / `ER_DUCKDB_THREADS` | Pod memory | `ER_DUCKDB_MEMORY_LIMIT` | Ephemeral storage |
|---|---|---|---|---|---|
| S | ≤100k | 2 | 6 GiB | 4GB | 40 GiB |
| M | ≤1M | 6 | 10 GiB | 6GB | 150 GiB |
| L | ≤10M | 12–16 | 14 GiB | 8GB | 400 GiB |

Memory grows a little because peak container usage was 9.37 GiB against a 4 GB DuckDB limit — the gap is Python heap, Arrow buffers, and the dbt subprocess, and it needs headroom. Ephemeral storage is sized from measured spill, not guessed.

Two constraints make these numbers inseparable from each other:

- **`ER_DUCKDB_THREADS` and the CPU request must be set in the same operation.** §2 records that DuckDB reads neither cgroup limits nor host core count, so a pod given 16 vCPU with `threads=2` wastes 14 of them, and a pod given `threads=16` on a 2-vCPU request thrashes. Neither fails loudly.
- **`ER_DUCKDB_MEMORY_LIMIT` must stay strictly below the pod memory limit**, which is the invariant [benchmarks/scales.py](../benchmarks/scales.py) already validates (`duckdb_memory_limit < mem_limit`). Exceeding it means the OOM killer rather than a graceful spill.

**The plumbing for this already exists and is half-wired.** `PATCH /v1/orgs/{org}/resources` ([server/src/erserver/api.py:451](../server/src/erserver/api.py)) accepts `duckdb_threads` (bounded `ge=1, le=32`) and `duckdb_memory_limit`, merges them into `orgs.env`, and the dispatcher resolves that env at **claim** time — so a change applies from the next job while a running one keeps its spawn env. What is missing is only: deriving the values from record count instead of an operator setting them by hand, and translating them into pod `resources` on the Job.

[benchmarks/scales.yaml](../benchmarks/scales.yaml) is the obvious source for that derivation rather than a new table — it already maps record counts to cpu/memory/duckdb envelopes and is validated by [benchmarks/scales.py](../benchmarks/scales.py). One caveat if it is promoted into a shared sizing table: its envelopes are deliberately generous so a benchmark never OOMs mid-run (the `1m` scale declares 12 CPU / 56 GiB), whereas production sizing should track measured *peaks*. The record-count-to-class mapping transfers; the memory figures should come from the measured column above.

Note the `le=32` bound on `duckdb_threads` caps the useful instance at 32 vCPU, which is what makes `c6id.8xlarge` the top of the ladder rather than anything larger.

**Two honest limits on this.** The thread-scaling measurement stops at 6 threads, so the knee above 6 is unknown — and it is unlikely to be linear, because the same run showed **reconcile getting 14% *slower*** at 6 threads, being a Python graph step that does not parallelize with DuckDB threads. Past some thread count the Python stages dominate and further vCPU buys nothing (§18.2).

And dynamic sizing makes the §2 API-pod inheritance problem worse, not better: if a tenant's env is set to 16 threads for its nightly 10M run, the API pods serving that tenant's read path inherit 16 threads and 8 GB across up to `_POOL_CAPACITY = 8` warm attaches. Per-run sizing should therefore be carried on the **job row**, not merged into `orgs.env`, until the [backend-design.md](backend-design.md) §8 split between tenant and API-pod settings is implemented (§18.10).

## 7. Data plane, per environment

### 7.1 Postgres: a pod in dev, Aurora in prod

**The DuckLake catalog is the least ephemeral thing in the system.** It holds table definitions, the snapshot log, and the mapping from table versions to parquet files. Lose it and the S3 objects are an unreadable pile: the parquet is re-derivable by re-running the pipeline from source, but the snapshot history and time travel that [backend-design.md](backend-design.md) sells as a differentiator are not. Nothing here is ever recreated from empty.

What it does *not* need is a managed database server standing up for one developer.

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

This shape satisfies the §2 shared-instance constraint trivially — the queue and the advisory lock are in the same Postgres instance by construction — which means the design holds whether or not that constraint turns out to be real (§18.1).

**Why not Aurora Serverless v2 with auto-pause in dev**, as the first draft specified: auto-pause requires *no active connections*, and §2 records that the dispatcher holds a permanent leader connection polling every two seconds. The cluster would never pause while any namespace was up, making the floor 0.5 ACU — about **$44/mo**, not the ~$5 the first draft's cost table assumed. The saving was illusory. A pod that scales to zero with its namespace (§5.2) delivers what auto-pause was supposed to.

A pod is also *closer* to tested parity than Aurora, which is counterintuitive but true: both compose files run `postgres:16` with `-c max_locks_per_transaction=1024`, so a pod matches what the integration tier exercises and Aurora is the configuration that diverges.

What dev gives up, all acceptable because prod is the system of record: PITR, managed backups, Multi-AZ, and cross-account snapshot sharing for the refresh path — dev uses `pg_dump` instead (§13). The EBS volume is AZ-bound, so the pod is pinned to one AZ; for dev that is fine, and it keeps the runner→catalog connection in-AZ.

Roles, in both forms: `er_app` for application traffic, `er_provisioner` with `CREATEDB` used only by `ERSERVER_MAINT_DSN`, and in prod `er_readonly` authenticated by IAM (`rds_iam`, so no password exists) for the read path in §8.4.

**Every DSN carries `sslmode=verify-full` in prod.** `sslmode` appears nowhere in the repository today, which is harmless against a localhost container and an encryption-in-transit gap against Aurora — and a question an auditor will ask (§8.6). It affects `ERSERVER_DSN`, `ERSERVER_MAINT_DSN`, `ERSERVER_TENANT_DSN_TEMPLATE`, `ERWEB_DATABASE_URL`, and the `ER_CATALOG_DSN` the engine receives, so it is a change to the secret values rather than to code — provided `verify-full` works with the Aurora CA bundle mounted in the images, which is the part to check rather than assume. Dev keeps `sslmode=disable` against the in-namespace pod.

### 7.2 The tenant database name collision

`tenant_db_name(org)` at [server/src/erserver/provision.py:79](../server/src/erserver/provision.py) returns `er_` + `t_<org>_<blake2b(org)[:8]>`. The hash covers the org name alone, so two developers who each provision an org called `acme` derive the same database name.

With Postgres per namespace (§7.1) this is no longer a collision — the databases are in different instances. **The fix is still worth shipping**, because it is what keeps a shared staging cluster and any future shared dev cluster viable: a new optional `ERSERVER_TENANT_DB_PREFIX`, defaulting to `er_` so current behaviour is bit-for-bit unchanged.

It deliberately prefixes the **database name only**, leaving `tenant_namespace()` — and therefore the metadata schema and the S3 prefix component — untouched, so a tenant's lake layout stays identical across environments. That is what makes the refresh path in §13 tractable.

### 7.3 S3 lake

One bucket per environment: `er-nonprod-lake`, `er-prod-lake`. Both with SSE-KMS under the environment's own CMK and a bucket policy denying non-TLS requests.

```
s3://er-nonprod-lake/dev/alice/{ns}/     ERSERVER_LAKE_DATA_PATH_TEMPLATE, per dev namespace
s3://er-prod-lake/{ns}/                  prod
```

`ERSERVER_LAKE_DATA_PATH_TEMPLATE` is validated to contain `{ns}` and end with `/` ([server/src/erserver/settings.py:55](../server/src/erserver/settings.py)); both forms satisfy it. The repository currently carries **three** inconsistent layouts — `s3://lake/er/` in the CI compose stack, `s3://lake/tenants/{ns}/` in the dev stack, and `s3://er-lake/{ns}/` as the settings default. Normalizing on the above is part of this work.

Models stay under the tenant's prefix (`provision.py` sets `model_uri_prefix = f"{plan.data_path}models/"`) so a tenant purge is one prefix delete.

**Versioning carries a lifecycle policy**, which the first draft omitted. Every pipeline run rewrites parquet, so versioning on a lake bucket without noncurrent-version expiration grows without bound. Noncurrent versions expire after 30 days, and incomplete multipart uploads after 7 — a run killed mid-write otherwise leaves parts that bill silently.

**CloudTrail data events are scoped, not blanket.** §8.5 relies on them to attribute reads of real records, but data events bill ~$0.10 per 100,000 and a DuckLake run issues an enormous number of object operations — a single 10M-row run could plausibly generate millions of events, which would make this the largest line in §16 rather than part of a rounding entry. The trail is therefore restricted to read events on the prod lake prefix only, excluding the nonprod bucket and excluding writes. §18.4 carries the measurement that would justify widening or narrowing it.

### 7.4 EFS

One filesystem per environment with an access point per namespace, mounted at `ERSERVER_CONFIG_ROOT` and `ERSERVER_DROP_ROOT` — by the API, the dispatcher, and every runner Job (§6.2). Elastic throughput. The content is org YAML and CSV import drops — a few gigabytes, so roughly $1/mo. This exists solely because those processes read and write the paths directly, and no object store substitutes without an application change.

**Backup is a checkbox, and ticking it is the decision.** EFS "automatic backups" is AWS Backup's default plan — daily, 35-day retention, $0.05/GB-month, so pennies here — and it is enabled. Worth being precise about what it protects: the org YAMLs are not actually at risk, because the full document lives in `config_versions.yaml` in the control-plane database and `provision.seed_config` already rewrites the file from the stored row on replay ([server/src/erserver/provision.py:218](../server/src/erserver/provision.py)) — the file is a projection of the DB. The only bytes unique to EFS are CSV drops awaiting ingest. The backup exists for those; before this revision the filesystem had no durability story at all, the one store in the system without one.

### 7.5 Email: SES

Email is not a future feature, which the earlier revisions implicitly treated it as. [server/src/erserver/mailer.py](../server/src/erserver/mailer.py) is a complete sender behind a durable `email_outbox` table that the dispatcher's leader tick drains with retry and backoff, and live flows depend on it: member invites, password reset, `job_failed` and `report_completed` notifications. It is dormant today only because nothing sets `ERSERVER_SMTP_URL` — which in a cloud environment means every invite parks silently in the outbox.

A `ses/` Terraform module provides the domain identity with Easy DKIM (three CNAMEs), a custom MAIL FROM (MX + SPF records), and a configuration set for bounce and complaint tracking. Pods reach the SMTP endpoint through the NAT on 587 — an SMTP interface endpoint exists but has availability-zone gaps in `us-east-1` and is not worth it at this scale, and port 25 is throttled from EC2 regardless.

Two facts carry lead time, which is why this lands in Phase 1 rather than whenever email first matters. Production access (sandbox exit) is a human-reviewed request with a stated ~24-hour first response — file it early. And the sandbox's limits — 200 messages a day, verified recipients only — are *fine for dev indefinitely*: verify the developers' own addresses and dev never needs the exit at all. The credential is §9.3's problem.

## 8. Security model

### 8.1 Identity Center permission sets

| Permission set | Account | Assigned to | Grants | Session |
|---|---|---|---|---|
| `PlatformAdmin` | nonprod | you + developers | Full nonprod. Mutating dev is unrestricted by design | 8h |
| `ProdDataReadOnly` | prod | you + developers | `s3:GetObject`/`ListBucket` on the prod lake, `kms:Decrypt` scoped `kms:ViaService = s3`, `rds-db:connect` as `er_readonly`, `eks:DescribeCluster` plus a `view`-only Kubernetes group | 4h |
| `ProdAdmin` | prod | **you only** | Full prod | 1h |

Access is granted by **group membership**, not by per-person assignment, so onboarding is one entry and offboarding is one removal.

No IAM users exist for humans, and no long-lived access keys exist anywhere except two service identities — S3 (§9.2) and SES SMTP (§9.3) — each carrying a scheduled Phase 4 retirement. The second was not in the earlier revisions; §9.3 explains why it cannot be designed away.

Two cautions on `ProdDataReadOnly`. The Kubernetes `view` role must never become `edit` — the built-in `view` excludes Secrets, which is the only thing keeping §9's projected prod secrets out of reach. And read access to prod **pod logs** is a PII path in a system whose whole purpose is handling personal records; the permission set grants `logs:` read for debugging, which is only safe because §10.4 makes it a rule that log lines carry identifiers and counts and never attribute values. That rule is what this grant depends on, and §17 verifies it rather than trusting it.

### 8.2 How prod writes are blocked in one account

*(Rev 4: the two guardrail statements previously lived in an SCP on the prod OU.
With a single account there is no OU and SCPs evaluate against nothing, so the
same two statements ride the permission sets as inline denies.)*

`PlatformAdmin` carries AdministratorAccess **plus two deny statements** scoped
to the prod prefix; `ProdAdmin` (1h sessions, prod-admins group only) is the
only permission set without them:

**Statement 1 — data mutation.** Denies `s3:Put*`/`Delete*` on `er-prod-*`
buckets, `rds:Delete*`/`Modify*`, `eks:Delete*`/`Update*`,
`secretsmanager:Put*`/`Delete*` and `kms:ScheduleKeyDeletion` on `er-prod-*`
/ `er/prod/*`-named resources.

**Statement 2 — privilege escalation.** Denies `iam:CreateRole`,
`PutRolePolicy`, `AttachRolePolicy`, `UpdateAssumeRolePolicy` and
`CreateAccessKey` against `er-prod-*` roles. Statement 1 leaves the
application's own `er-prod-*` roles writable paths to the lake, so statement 2
is what keeps a developer from minting `er-prod-app-anything` and walking out
of the guardrail — the exemption is only trustworthy because minting the thing
it exempts is itself denied.

**The enforcement difference from the SCP version is honest and material.** An
SCP sits above the identity-policy layer and catches a wrongly written IAM
policy; an inline deny *is* the identity-policy layer. Whoever can edit
permission sets can remove the guard — one policy edit, not a structural
boundary. The operator accepted this trade (§3). Mitigations, all required for
the claim to mean anything: the permission sets live in Terraform behind
`main`'s branch protection (§8.6), so removing a deny is a reviewed, logged
diff; `ProdAdmin` assumption is alerted on (§8.6); and prod-mutating IAM paths
are tested by the §17 probes exactly as the SCP was — the probe asserts the
deny, not the mechanism.

The Terraform exemption survives unchanged: whoever can assume the Terraform
apply role walks through the guardrail legitimately, so the role's **trust
policy** is part of this section's claim — §14.2 pins it to the CI apply role
on `main` and `ProdAdmin` as break-glass.

### 8.3 Customer-managed KMS keys, from day one

Every S3 bucket and every Aurora cluster gets a customer-managed key, for two specific reasons:

- An RDS snapshot encrypted with the default `aws/rds` key **cannot be shared across accounts** at all.
- A nonprod principal cannot read an SSE-KMS prod object unless the prod key policy grants it.

Retrofitting a CMK onto a live prod database means a full snapshot-copy-and-restore migration with downtime. Doing it at creation costs nothing. The prod lake key grants `kms:Decrypt` to the nonprod refresh role; the prod Aurora key grants `kms:Decrypt` and `kms:CreateGrant` so a shared snapshot can be restored.

ECR is the deliberate exception — AES256, because images are built from public source, hold no records, and a CMK would require a key plus cross-account grants in every replica account for no confidentiality gain.

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

**NetworkPolicy must be explicitly enabled to do anything.** The per-namespace NetworkPolicy in §12 is inert under the Amazon VPC CNI unless the network policy agent is turned on in the addon configuration. Without it you get objects that look like isolation and enforce nothing — between namespaces that now hold real records. This is a one-line addon setting and a silent failure if missed, which is why §17 verifies it behaviourally rather than by inspection.

**Attribution has two layers.** `X-Acting-User` is spoofable by any API-key holder by design, so person-level attribution comes from `erweb.audit`, not erserver's `audit_log`; CloudTrail data events on the prod lake (§7.3) are the backstop for data access.

One note to save someone an afternoon: `ERWEB_SESSION_SECRET`, despite the name and the 32-character minimum in `env.ts`, is **never read by any code** — sessions are DB-backed opaque tokens hashed at rest. It needs a value to pass validation; it does not need a rotation policy.

### 8.6 SOC 2 readiness

A first SOC 2 is expected ([backend-design.md](backend-design.md) §15 phase 4 already names "SOC2 groundwork"), so the question for this document is what to do *now* that cannot be done later.

**The useful sorting principle: SOC 2 Type 2 rarely fails on architecture, it fails on evidence.** Controls must be shown to have *operated* across an observation window, typically three to six months. Almost every control below can be added the week before an audit. Three cannot, because they have a time component — you cannot retroactively produce six months of logs, and you cannot make a log immutable after someone has had the ability to delete it.

#### Cannot be retrofitted — build in Phase 0

**1. Retention must span the observation window.** §10.4 specifies 30 days of log retention, chosen on cost grounds. For a six-month Type 2 that leaves five months with no evidence. Audit-relevant trails need **≥ 400 days** (a year plus margin, so a window never runs off the end):

| Trail | Retention | Why it is audit-relevant |
|---|---|---|
| CloudTrail (management + data events) | 400 days | The primary evidence for CC6 access control and CC7 monitoring |
| `erweb.audit` (person-level) | 400 days | Who did what in the application — the only person-level attribution there is (§8.5) |
| erserver `audit_log` (credential-level) | 400 days | API-key-level actions |
| Application debug logs | 30 days | Not audit evidence. Keep the cost control here |

The split matters: retention is a **compliance parameter for audit trails and a cost parameter for everything else**, and conflating them is what makes people either overpay or fail the window. §10.4 is updated accordingly.

**2. Audit logs must land where no one with workload access can delete them.** In the rev 4 single account there is no "elsewhere" to deliver to, so the control is carried entirely by the bucket itself: versioning, **S3 Object Lock in compliance mode at 400 days** — a retention not even the root user can shorten or override — plus a policy denying deletes to everything but the account root. A developer can see the archive; nobody, including the principals the logs record, can remove an object before its retention expires. (The standard answer remains a dedicated log-archive account; it is the first thing to reinstate if the account split returns — §3.)

**3. CloudTrail log file validation, enabled from the start.** This is the specific tamper-evidence mechanism auditors ask about — CloudTrail writes signed digest files so you can prove the log was not altered. It only validates logs written *after* it is enabled.

**4. Access-change history, which this design already gets free.** Identity Center group membership lives in Terraform (§8.1), so the quarterly access review that SOC 2 CC6.2 expects is a git log plus PR approvals rather than a spreadsheet someone has to remember to produce. Worth stating explicitly because it converts a recurring manual control into an automated one — provided `main` has branch protection with required review, which is itself a CC8 change-management control. Both should be true before the window opens, not during it.

#### Can be retrofitted — defer until an auditor is engaged

Written policies, the risk assessment, vendor assessments, security training records, incident-response runbooks, and penetration testing. These are real work but they are point-in-time artifacts; building them now means maintaining them through eighteen months of design churn.

#### What this design already satisfies

Worth knowing so effort does not go where it is not needed. The access-control criteria (CC6) are the largest block and this design is unusually well placed: no IAM users for humans, no long-lived access keys, MFA enforced, short sessions, least privilege through permission sets, and prod mutation blocked by the §8.2 permission-set denies (IAM-layer since rev 4 — weaker than the prior SCP posture; §3 records the accepted trade). Encryption at rest is customer-managed throughout (§8.3), and TLS is enforced by bucket policy (§7.3). Change management (CC8) is covered by Terraform in git, tests on every PR, and §11's promote-by-digest — an immutable tag plus a digest means you can *prove* what ran in prod came from a specific reviewed commit, which is better evidence than most organisations can produce.

#### Two things that are findings today

**Committed development credentials.** §8.5 already covers this as a security matter; it is also the first thing an auditor greps for. `frontend/dev/env.sh` hardcodes `ERWEB_CREDENTIAL_KEY` and `ERSERVER_OPERATOR_TOKEN`, and `admin@dupezone.com / asdfasdf` is seeded by [frontend/dev/create-users.mjs](../frontend/dev/create-users.mjs). The §8.5 rule — local Compose only, never an environment holding real records — is what makes this defensible, and it needs to be true in fact, not just in this document.

**The AI assistant is an undocumented subprocessor relationship.** [frontend/src/lib/assistant/tools.ts](../frontend/src/lib/assistant/tools.ts) gives the model `search_golden_records` ("Search golden records by name or email") and `get_entity` ("golden values, member records per source, per-field lineage, event history"). So a shipped feature sends real customer personal data — names, emails, full golden records — to the Anthropic API via `ANTHROPIC_API_KEY`.

That is not a problem, but it is a **subprocessor** processing customer personal data, and it needs inventorying, a data processing agreement, a configured data-retention posture on the vendor side, and disclosure to customers. It is the kind of thing that surfaces late and awkwardly because it is a product feature rather than an infrastructure decision. It also strengthens §10.5's recommendation to keep *logs* in-account: every SaaS telemetry backend is another subprocessor to assess, and traces and metrics carry no record values under §10.4's rule while logs might.

#### Standing production read access

§8.1 grants every developer `rds-db:connect` as `er_readonly` plus read on the prod lake — standing access to all production customer records. That is a deliberate decision (it is why §8 exists in this shape), and it is defensible under SOC 2, but only with the controls that make it so: a documented business justification, CloudTrail data events attributing every read (§7.3), and ideally time-boxed rather than standing grants. An auditor will ask why it is standing; "because developers need to debug real data" is an acceptable answer when logged and justified, and an unacceptable one when neither.

A related point in the other direction: `ProdAdmin` assigned to one person is good least-privilege and weak segregation of duties — one individual can do anything in prod with no second approval. The cheap mitigation is **alerting on `ProdAdmin` role assumption**, so the action is at least visible to someone, plus a documented break-glass procedure (§15, Phase 4).

#### AWS services worth enabling as evidence generators

| Service | Rough cost | Value |
|---|---|---|
| **AWS Config**, narrow recording scope | a few $/mo | Records configuration over time — the evidence for "was encryption enabled throughout the window", which is otherwise hard to prove retrospectively |
| **GuardDuty**, prod only | ~$5–30/mo | Threat detection. Cheap, and its absence is a common finding |
| Security Hub with the AICPA SOC 2 standard | ~$10/mo, noisy | Useful closer to an audit; premature now |

Config is the one with a time component, so it belongs in Phase 0 with the trails. The other two can wait.

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

The first draft said Aurora passwords "use Secrets Manager managed rotation" and left it there. That combination breaks the platform: a process environment is fixed at exec time, so on rotation ESO updates the Kubernetes Secret while the running pods keep the stale DSN and connections begin failing.

Two parts to the fix. A **reloader** — annotate the Deployments so a change to the projected Secret triggers a rolling restart. And for the dispatcher, which is `replicas: 1` / `strategy: Recreate` (§2), a restart is a brief gap in job dispatch rather than a seamless rollout, so rotation is scheduled rather than arbitrary.

**`ERWEB_CREDENTIAL_KEY` must not be rotated automatically at all.** It unseals every org's stored erserver API key in `erweb.org_credentials` (AES-256-GCM with AAD `"<org>\0<role>"`, per [frontend/src/lib/db/crypto.ts](../frontend/src/lib/db/crypto.ts)); rotating it without a re-encrypt pass bricks the vault. Documented as manual-with-migration, deliberately not wired to a schedule.

### 9.2 S3 credentials: static keys now, IRSA later

`ObjectStore.from_env()` in [src/er/lake/objectstore.py](../src/er/lake/objectstore.py) and `attach_statements()` in [src/er/lake/ducklake.py](../src/er/lake/ducklake.py) both require a literal `ER_S3_ACCESS_KEY_ID` / `ER_S3_SECRET_ACCESS_KEY` and a non-empty scheme-less `ER_S3_ENDPOINT`. There is no credential-chain path, no `AWS_*` support, and no way to omit the endpoint — `require_env` rejects empty values deliberately, because an empty secret attaches a lake that authenticates as nobody.

**Phase 1 (no code change).** One IAM user per environment with a lake-scoped policy, keys in Secrets Manager, with `ER_S3_ENDPOINT=s3.us-east-1.amazonaws.com`, `ER_S3_URL_STYLE=vhost`, `ER_S3_USE_SSL=true`. This works against real S3 today. Rotation here has the same restart requirement as §9.1.

**Phase 4 (the right answer).** A credential-provider mode so DuckDB uses `CREATE SECRET (TYPE s3, PROVIDER credential_chain)` and boto3 uses its default chain, both picking up the IRSA web-identity token, after which no static S3 key exists anywhere — and the §9.1 rotation problem disappears for S3 entirely.

Scheduled separately rather than bundled into bring-up because it touches [src/er/lake/ducklake.py](../src/er/lake/ducklake.py), [src/er/lake/objectstore.py](../src/er/lake/objectstore.py), and [dbt/profiles/profiles.yml](../dbt/profiles/profiles.yml) together, and `tests/unit/test_dbt_profiles.py` contract-tests those three as field-for-field identical.

### 9.3 SES SMTP: the second static credential, and why no rotation story fixes it

[server/src/erserver/mailer.py](../server/src/erserver/mailer.py) speaks SMTP — stdlib `smtplib` behind `ERSERVER_SMTP_URL`. SES's SMTP interface **does not accept credentials derived from temporary credentials**; AWS documents this flatly, so IRSA can never back the SMTP path, no matter how the cluster identity work in §9.2 progresses. Until the mailer is ported to the SES API (Phase 4, alongside §9.2's credential chain — same claim, same retirement shape), a second IAM user exists: `ses:SendRawEmail` only, SMTP password derived from its access key, stored in Secrets Manager, rotated under §9.1's reloader-and-restart rules. This is the amendment to §8.1's "one long-lived key" claim, and the honest accounting is the point: two static keys, both with named exits.

## 10. Observability

Nothing currently leaves the box. There is no metrics endpoint, no tracing, and no log aggregation — [backend-design.md](backend-design.md) §14 specifies what should exist (pod logs tagged `tenant/job/run/stage`, API RED metrics, queue depth, dispatch latency, per-class concurrency) and none of it is wired, mostly because there has been no cluster to wire it to.

**OpenTelemetry is the right foundation here**, for four reasons specific to this system rather than general enthusiasm:

1. **Three runtimes, one standard.** The engine is Python, the control plane is Python/FastAPI, the BFF is Node/Next.js. OTel is the only instrumentation standard that covers all three over one wire protocol into one collector.
2. **The interesting unit of work crosses five process boundaries.** A user clicking "run" traverses Next.js → BFF → erserver API → a `jobs` row → the dispatcher → a Job pod → a dbt subprocess → DuckDB. Answering "why was this run slow" without propagated trace context means hand-correlating five log streams by `run_id`. Distributed tracing is the one thing that is genuinely hard to retrofit, and §6 makes it harder by putting the runner in a different pod.
3. **Vendor neutrality is a product requirement, not a preference.** [backend-design.md](backend-design.md) §15 phase 5 contemplates a self-hosted option. A collector lets a self-hoster point at their own backend; CloudWatch-native instrumentation does not.
4. **The engine already emits the right shape.** Stage records carry `tenant/job/run/stage` and typed counters; they become span attributes almost directly.

### 10.1 What already exists, and the boundary not to cross

| Exists | Where | Role |
|---|---|---|
| `runs`, `run_stages` with typed counters and snapshot ranges | [src/er/obs/runctx.py](../src/er/obs/runctx.py) | **Durable business telemetry, already in Postgres** |
| One JSON record per stage on stderr | engine S5.2 records | Live progress, consumed into `jobs.progress` |
| Opt-in profiling: `ER_PROFILE_SQL`, `ER_PROFILE_PYTHON`, `ER_PROFILE_STORAGE` → `ER_PROFILE_DIR` | [src/er/obs/sql_profile.py](../src/er/obs/sql_profile.py), [docs/profiling.md](profiling.md) | Deep per-query and per-stage attribution, off by default |

**`runs`/`run_stages` stays the system of record for business counters** — how many golden records, how many candidate pairs, which snapshot range. OTel metrics are for *operational* questions: is the queue backing up, are pods being evicted, what is p95 dispatch latency. Emitting row counts as OTel metrics as well would create two answers to the same question with different retention and different accuracy. The rule: anything a user might see in the UI comes from Postgres; anything an operator pages on comes from OTel.

### 10.2 Traces — and the one schema change that makes them work

The runner is a separate pod launched asynchronously from a database row (§6), so trace context has to survive that hop. It does not travel by itself.

**Add `jobs.trace_context` (text, nullable)** holding the W3C `traceparent` captured at enqueue. The runner reads it and starts its root span as a child of that context. This is the highest-value single decision in this section: with it, one trace spans the click, the API call, the queue wait, the dispatch, every pipeline stage, and the dbt invocations. Without it you get two unrelated traces and a `run_id` to join them by hand.

Instrumentation, in rough order of value per unit of effort:

- `opentelemetry-instrumentation-fastapi` and `-psycopg` on the API and dispatcher — near-zero effort, immediately gives RED metrics and DB span timing.
- **A span per pipeline stage** in the runner. [src/er/obs/runctx.py](../src/er/obs/runctx.py) already brackets exactly these boundaries and already holds the counters that become span attributes, so this is an adapter rather than new instrumentation.
- `@vercel/otel` in the BFF, propagating into the typed erserver client.
- The queue wait as its own span — the gap between enqueue and claim is where per-tenant serialization shows up, and it is invisible today.

Sampling: **always sample pipeline runs.** [backend-design.md](backend-design.md) §5 puts volume at hundreds of jobs per day, which is nothing, and each one is high-value. Head-sample the API read path, which is where the volume actually is.

**One gotcha specific to Jobs.** The OTel SDK's batching exporter drops unflushed spans when a process exits, and a runner pod exits immediately after its final span — so the tail of every run, including the failure path, would silently vanish. The runner needs an explicit `force_flush()` with a timeout in its exit path, on every exit code. This is exactly the sort of thing that looks fine in dev (where runs are short and the batch window is wide relative to them) and loses the most interesting spans in production.

### 10.3 Metrics — and the cardinality trap

[backend-design.md](backend-design.md) §14's list maps cleanly: API RED, queue depth, dispatch latency, per-class concurrency. Worth adding, because they validate claims this document makes and cannot currently check:

- **Spill bytes per stage** — §18.2 needs this measured anyway, so instrumenting it serves the design and the operations at once.
- **Pod eviction and OOM-kill counts**, which is how §5.3's ephemeral-storage sizing proves correct or not.
- **Spot interruption rate and Karpenter node count**, which is how §5.2's suspend-to-zero claim and §16's spot pricing assumption get verified rather than asserted.

**Do not put `tenant` on metric series.** This is the classic multi-tenant SaaS cost blowup: every distinct label value is a separate time series, tenant count is unbounded by design, and the bill scales with tenants × metrics × resolution. Use `resource_class` as the dimension on metrics, and get per-tenant detail from traces and the `runs` table, where tenant is an attribute on a sampled record rather than a permanent series. The engine's own `tenant` tagging is correct for logs and spans and wrong for counters.

### 10.4 Logs, and the PII rule

The engine already writes structured JSON to stderr, which is the hard part done. Add `trace_id` and `span_id` to those records so a log line links to its span.

**This is a deduplication engine, so log content is a privacy question, not a formatting one.** Names, emails, phone numbers, and addresses are the payload being processed. The rule: **log lines carry identifiers and counts — `record_key`, `entity_id`, `tenant`, row counts — never attribute values.** A redaction processor in the collector is a backstop, not the control; the control is not emitting them. This supersedes the open item that previously asked whether prod logs contain record values — the answer is that they must not, and §17 verifies it.

Volume is a real cost, not a rounding error: dbt is chatty, and a 10M-row run's dbt output alone can reach hundreds of megabytes. CloudWatch Logs ingestion is ~$0.50/GB. So log level is environment-configured (dbt at `warn` in prod, `info` in dev).

**Retention is split by purpose, because the two kinds of log answer to different masters.** Debug output is a cost problem: 30 days, then expiry. Audit trails are a compliance problem and need to outlast an audit observation window — **400 days** for CloudTrail, `erweb.audit`, and erserver's `audit_log` (§8.6). Applying the 30-day figure uniformly, as the first draft did, would leave a six-month Type 2 audit with five months of missing evidence and no way to recover it.

### 10.5 One pipe, one destination to start, and the split that comes later

Two decisions get conflated here, and they have different answers.

**One collection layer: yes, unambiguously.** Every deployable speaks OTLP to one collector, which batches, redacts, samples, and routes. This is the decision worth getting right first because it is the one embedded in application code — changing it later means touching every service.

**One storage backend for all three signals: no, and deliberately not.** The signals have genuinely different storage shapes — metrics are small, regular, aggregated, and want long retention; logs are large, bursty, and want full-text search with short retention; traces want ID lookup and service-graph queries over sampled data. Nothing serves all three well: Grafana's LGTM stack is three storage engines behind one UI, and CloudWatch covers logs and metrics while X-Ray is a separate service for traces. "One system" realistically means one query surface over three stores, and the collector is what makes that an operational choice rather than an architectural one.

**Do not derive metrics from logs.** CloudWatch metric filters and Logs Insights aggregations make this easy and it is a trap: you pay log-ingest rates for every data point, on a system where dbt alone can emit hundreds of megabytes per 10M-row run. Metrics are emitted as metrics.

#### Topology

An **OTel Collector as a Deployment (gateway)**, with a **DaemonSet** added for log collection. Both are needed, and for opposite reasons:

- **Traces and metrics push over OTLP** to the gateway, with `force_flush()` on exit (§10.2).
- **Logs stay on stdout/stderr and are collected by the DaemonSet's `filelog` receiver.** This is the more reliable path for an ephemeral Job specifically: the container runtime captures both streams regardless of how the process exits, so logs survive exactly the crash that loses unflushed OTLP spans. The engine already writes its structured stage records to stderr and its one result line to stdout, and the runtime captures both into the same log stream — so this needs no application change beyond adding trace ids to the JSON.

#### Backend

| Option | Cost direction | Trade-off |
|---|---|---|
| **ADOT → CloudWatch + X-Ray** | Cheapest to start, most expensive to scale | No new infrastructure; fits the §8 IAM and account model. Logs Insights is poor at high-cardinality exploration and X-Ray's trace model is weaker than Tempo's |
| AMP + Managed Grafana + X-Ray | Moderate, predictable | Much better querying, data stays in-account, two more services to wire |
| SaaS (Grafana Cloud, Honeycomb free tiers) | Free at this volume | Best querying, zero operations — but data leaves the account, which for *logs* conflicts with §8.5 |
| Self-hosted LGTM in-cluster | Cheapest at high volume | **Disqualified here.** Four stateful services to operate and back up for a two-person team, running on the same cluster and competing for the nodes §5.3 sized for 141 GiB of spill. An observability stack that degrades when the system it observes gets busy is worse than none |

**Start with CloudWatch and X-Ray via ADOT.** Not because it is the best backend — it is not — but because the argument for OTel is that the exporter is swappable, and the cost at one developer is a few dollars. Choosing the boring option first is the cheapest way to discover what you actually need to query.

The asymmetry to watch: CloudWatch Logs at ~$0.50/GB ingested is a rounding error at one developer and a top-three line item at a hundred tenants running nightly (§16). That is the signal to move, and moving is an `exporters` change.

**The known next step is splitting by signal**, which only the collector makes cheap: logs stay in CloudWatch, where they are in-account and retention is easy; metrics and traces go to Grafana Cloud or AMP+Grafana for the querying. The split follows the PII boundary rather than convenience — logs are the signal that could carry record values, so they are the one that must not leave the account, while metrics and traces carry none under §10.4's rule. Worth knowing it is available; not worth building on day one, because two places to look is a real cost for two people.

### 10.6 Phasing, and the coupling to §6

This should not land at once. It also should not land *after* the Job launcher, because the two share a seam.

- **Phase 1:** collector deployed; logs shipped with trace fields; FastAPI/psycopg auto-instrumentation for API RED metrics. Cheap, immediate, no schema change.
- **Phase 2, alongside the Job launcher:** `jobs.trace_context`, spans per stage, `force_flush()` on runner exit. **Designed together with §6.3's progress decision** — both cross the dispatcher-to-pod boundary that the subprocess pipe used to cover for, and solving them separately means building that bridge twice.
- **Phase 4:** `ER_PROFILE_*` output as span events, dashboards for the §14 metric list, alerting.

## 11. Container registries

Three images, two of which do not exist yet:

| Image | Status | Contents |
|---|---|---|
| `er-pipeline` | exists — [docker/Dockerfile](../docker/Dockerfile) | The runner. Baked DuckDB extensions, dbt, and compiled MinIO binaries |
| `er-api` | **new** | Slim: `src/er` + `server/src/erserver`, no dbt, no MinIO. Serves both `er-api` and `er-dispatcher` |
| `er-web` | **new** | Next.js standalone output on Node 22 |

ECR repositories live in the account's single registry, with tag immutability, scan-on-push, and a lifecycle policy retaining the last 30 images. *(Rev 4: the prior nonprod-source-of-truth → prod-replica arrangement collapses with the accounts; replication and the destination registry policy are no longer needed.)*

The byte-identity guarantee survives without replication because it never depended on it: **promotion is by digest, never by tag.** A prod deploy pins the exact digest CI built and tested; an immutable tag plus a digest proves what ran in prod came from a specific reviewed commit.

CI gains a GitHub OIDC provider and a push job replacing today's `load: true`. CI currently consumes zero repository secrets and declares only `permissions: contents: read`; the push job adds `id-token: write` and assumes `er-ci-ecr-push`. The trust policy restricts the `sub` claim to pushes on `main` — **pull requests are excluded deliberately**, since a PR from a fork runs attacker-authored workflow code and must never hold a push credential.

`er-pipeline` shipping a compiled MinIO *server* binary is worth revisiting: it exists because MinIO's registries stopped serving the pinned community release and compose needs an object store, but a production runner has no use for it and it is avoidable attack surface.

### 11.1 Cold nodes pull the whole image, every time

The second revision said to pin the tag "and let the node cache do the work." For the runner pools there is no node cache: §5.1 empties a runner node ~30 seconds after its pod exits, so nearly every run starts on a freshly provisioned node pulling `er-pipeline` from scratch. The bytes are already cheap — layer blobs come from S3 over the gateway endpoint (§4) — but the minutes are real on an image that carries a full venv (splink, pyarrow, duckdb, dbt), baked DuckDB extensions, compiled MinIO binaries, and — because the compose `pipeline` service's command *is* pytest — the tests, fixtures, and dev dependency group too.

The mitigation ladder, cheapest first, climbed only as far as measurement says to:

1. **SOCI parallel pull — effectively free.** The snapshotter ships in current EKS AL2023 and Bottlerocket AMIs (and is on by default under Auto Mode), with up to ~60% faster pulls on 10 GB-class images for zero configuration.
2. **A runtime/CI image split.** The runtime target drops `tests/`, `fixtures/`, `benchmarks/`, the dev group, and the MinIO server binary §11 already calls avoidable attack surface. This is a contract change, not just a Dockerfile change: `test_dockerfile_contract.py` exists to keep the compose image testable, so the split means two targets and an updated contract test — the guarantee moves, it does not disappear.
3. **Snapshot-prebaked images.** Bottlerocket's data volume accepts a `snapshotID` in the EC2NodeClass `blockDeviceMappings`, with `aws-samples/bottlerocket-images-cache` doing the baking. Standard Karpenter only — unavailable under Auto Mode (§5.5) — and it adds an artifact to keep fresh, so it is the last rung, not the first.

The deciding measurement — cold-node pull time for the current image — does not exist yet (§18.16).

## 12. Per-developer environments

A developer's environment is a namespace plus its own Postgres and S3 prefix, driven by Helm so that it takes seconds and holds no Terraform state:

```
make dev-up DEV=alice
  1. helm upgrade --install alice charts/er-dev-namespace \
       --set developer=alice --set lakePrefix=dev/alice
     → Namespace, ResourceQuota, LimitRange, NetworkPolicy,
       Postgres StatefulSet + PVC, EFS PVC, IRSA ServiceAccount
       scoped to s3://er-nonprod-lake/dev/alice/*, ExternalSecret,
       three Deployments, Ingress at alice.dev.<domain>
  2. wait for postgres-0, then CREATE DATABASE erctl (via er_provisioner)
  3. drizzle migrate + ensure_schema
  4. seed
```

Secrets come from `er/dev/alice/{erserver,erweb}`, created by the same target if absent.

Three details carry over from the local stack. The dispatcher gets `replicas: 1`, `strategy: Recreate`, and an init container gating on the API's `/healthz`, because its leader connection otherwise blocks `ensure_schema` indefinitely. The Postgres pod runs with `-c max_locks_per_transaction=1024`, matching both compose files. And `ResourceQuota` is what stops one developer's 10M-row run from starving the cluster — `requests.memory: 80Gi`, `limits.cpu: 24`, `count/jobs.batch: 4`, and **`requests.ephemeral-storage`**, without which §5.3's spill sizing is unenforced.

Two details are cloud-only. The per-developer Ingresses share **one internal ALB** via the controller's `group.name` (§4.1); the controller's documented warning — anyone with Ingress RBAC in the group can override its rules — describes, in dev, people who already hold `PlatformAdmin`, so the trust boundary is unchanged, and the same reasoning is why prod never joins a dev group. external-dns publishes `alice.dev.<domain>` against the wildcard certificate, scoped with `--domain-filter=dev.<domain>` and a per-cluster `--txt-owner-id` so it can neither touch another zone nor fight another cluster over records. And step 3's drizzle migrations run as a Helm pre-upgrade hook Job: `node scripts/db-migrate.mjs` already exists as a clean container entrypoint reading `ERWEB_DATABASE_URL`, while `ensure_schema` rides the API's boot exactly as it does locally, dispatcher init-container ordering unchanged.

Lifecycle:

```
make dev-suspend DEV=alice   # Deployments + StatefulSet → 0. PVC and S3 kept.
make dev-resume  DEV=alice   # back up in under a minute
make dev-down    DEV=alice   # + delete PVC, DROP DATABASE ... WITH (FORCE),
                             #   delete the S3 prefix, delete the secrets
```

Suspend is the cost control (§5.2) and `dev-down` is the purge path [backend-design.md](backend-design.md) §7 designed for. The distinction matters: suspend keeps the catalog, so resuming does not mean another refresh.

## 13. Moving production data to lower environments

Lower environments take real records with no scrub step. The constraint is that a **full-fidelity lake clone is not supported by the engine today**: `er init` records `DATA_PATH` in the catalog and the immutability check at [src/er/lake/init.py:180](../src/er/lake/init.py) raises `PreconditionFailure` (exit 3) on mismatch. Restoring a prod catalog into nonprod yields a catalog still pointing at `s3://er-prod-lake/...`, and no supported operation re-homes it.

**Path A — export and re-ingest.** What this design builds.

```
make refresh-dev DEV=alice ORG=acme
  1. assume prod/er-data-export
  2. read prod source + golden records via DuckDB over S3 → Parquet
  3. write to s3://er-nonprod-lake/dev/alice/_import/
  4. er ingest in Alice's namespace, then run_all_full
```

Full raw fidelity on the records, using only supported engine paths. It loses prod's snapshot history and costs a pipeline run — roughly 90s at 1M records, or about 1h04m at 10M with 6 threads ([performance.md](performance.md)).

Because dev Postgres is a pod rather than Aurora (§7.1), the catalog side of a refresh is `pg_dump`/`pg_restore` rather than a snapshot share. For a single tenant's catalog that is the simpler path anyway.

**Path B — `er lake rehome`.** A new engine command that updates the recorded `DATA_PATH` and rewrites catalog file references, reducing refresh to an `aws s3 sync` plus a catalog restore: minutes instead of an hour, with snapshot history preserved. Raised as its own issue, because the work depends on whether DuckLake stores absolute or relative paths in the catalog — worth determining early, since it decides whether Path A is permanent or temporary (§18.7).

Cross-account Aurora snapshot plumbing is still built for staging refresh and prod DR, just not for the per-developer path.

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

Bootstrap is two stacks, not one, because the state bucket must live in an account and a provider that assumes into an account created by the same apply has an unknown `role_arn` at plan time.

Terraform state lives in S3 in the account with `use_lockfile = true` — native conditional-write locking, no DynamoDB table. The bucket is versioned (the only recovery path from a truncated state file) and SSE-KMS encrypted under its own CMK, because state holds database endpoints and every output marked sensitive.

*(Rev 4 note: the prior revision's trade-off — prod state living in the nonprod account — dissolved with the accounts. What replaces it is the broader §3 trade: state, like everything else, is protected by role policy rather than an account boundary.)*

### 14.1 Application changes this requires

| Change | Files | Why |
|---|---|---|
| `er-api` image | new `docker/Dockerfile.api` | Slim, no dbt or MinIO; [backend-design.md](backend-design.md) §4 specifies it |
| `er-web` image | new `frontend/Dockerfile` | None exists; the BFF runs as a host process today |
| `ERSERVER_TENANT_DB_PREFIX` | [server/src/erserver/provision.py](../server/src/erserver/provision.py), [settings.py](../server/src/erserver/settings.py) | Shared-cluster tenant isolation (§7.2) |
| CI push job | [.github/workflows/ci.yaml](../.github/workflows/ci.yaml) | OIDC + `push: true`, adding `id-token: write` |
| Normalize the lake prefix | [configs/default.yaml](../configs/default.yaml), [frontend/dev/env.sh](../frontend/dev/env.sh) | Three inconsistent layouts today (§7.3) |
| **k8s Job launcher** behind the existing seam | `launch_runner` in [server/src/erserver/dispatcher.py](../server/src/erserver/dispatcher.py) | Replaces `subprocess.Popen` with a Job + watch (§6.2). The seam exists for exactly this |
| **Runner writes its own progress** | [server/src/erserver/runner.py](../server/src/erserver/runner.py), [queue.py](../server/src/erserver/queue.py) | A Job pod has no stderr pipe to the dispatcher (§6.3) |
| **Per-run resource sizing** from record count | [server/src/erserver/queue.py](../server/src/erserver/queue.py), [policy.py](../server/src/erserver/policy.py), derived from [benchmarks/scales.yaml](../benchmarks/scales.yaml) | CPU-tiered classes on the job row, not `orgs.env` (§6.5) |
| **`jobs.trace_context`** column | [server/src/erserver/db.py](../server/src/erserver/db.py) | W3C traceparent across the enqueue→pod hop; one trace end to end (§10.2) |
| **OTel instrumentation + `force_flush()` on exit** | [server/src/erserver/api.py](../server/src/erserver/api.py), [runner.py](../server/src/erserver/runner.py), [src/er/obs/runctx.py](../src/er/obs/runctx.py), `frontend/` | Spans per stage; an exiting Job otherwise drops its last batch (§10.2) |
| **Per-run cost on the run row** | [server/src/erserver/queue.py](../server/src/erserver/queue.py), [src/er/obs/runctx.py](../src/er/obs/runctx.py) | vCPU-hours × node rate, recorded next to the existing counters so cost-per-tenant is one SQL join (§16.2) |
| Non-root `USER` in all three images | [docker/Dockerfile](../docker/Dockerfile) (none today), both new Dockerfiles | Pod Security `restricted` requires it; a root engine process in a PII cluster is unforced risk (§5.6) |
| Disable FastAPI docs routes outside dev | [server/src/erserver/api.py](../server/src/erserver/api.py) | `/docs`, `/redoc`, `/openapi.json` are the only unauthenticated routes besides `/healthz` (§4.1) |
| *(optional)* `api_keys.last_used_at` | [server/src/erserver/db.py](../server/src/erserver/db.py), [auth.py](../server/src/erserver/auth.py) | Key authentication currently writes nothing; a last-used stamp turns the SOC 2 credential review into a query (§8.6) |
| *(Phase 4)* S3 credential chain | [src/er/lake/ducklake.py](../src/er/lake/ducklake.py), [objectstore.py](../src/er/lake/objectstore.py), [dbt/profiles/profiles.yml](../dbt/profiles/profiles.yml) | Drop static keys for IRSA; contract-tested together (§9.2) |
| *(Phase 4)* mailer → SES API | [server/src/erserver/mailer.py](../server/src/erserver/mailer.py) | Retires the SMTP IAM user: SMTP credentials cannot be temporary, the API path can be IRSA (§9.3) |

This is a longer list than the first draft's, and most of the additions come from one source: putting the runner in its own pod removes the stderr pipe the dispatcher currently relies on, and that single change is what pulls in progress reporting, trace propagation, and flush-on-exit together. They should land as one piece of work (§10.6).

### 14.2 How a change reaches an environment

The earlier revisions defined what runs and never who applies it — and the gap was load-bearing, because two §8 claims quietly depend on the answer: promote-by-digest is only CC8 *evidence* if the deploy path enforces it, and the SCP exemption for Terraform is only safe if assuming the Terraform role is harder than the SCP it bypasses.

**Terraform.** Plan on every PR under a read-only role; apply on `main` under the apply role — both assumed through §11's existing GitHub OIDC provider, with the same `main`-only `sub` restriction the ECR push role carries, and for the same reason: a PR from a fork runs attacker-authored workflow code. The prod-scoped apply role's trust policy names exactly two principals — the CI apply role and `ProdAdmin` as break-glass. That trust policy is the §8.2 guardrail's last clause.

**Helm.** Dev platform releases apply from CI on `main`; `make dev-up` stays a human command, because a developer namespace is that developer's to create and destroy. Prod deploys are manually triggered, pass an image **digest** — never a tag — and gate on that digest being present in the registry first. Argo CD remains out of scope, as §6.1 noted; this section is the minimum that turns promote-by-digest from a convention into a property.

## 15. Build order

**Phase 0 — Foundations.** The account baseline (root MFA, S3 BPA, billing access), Identity Center permission sets and groups — including the §8.2 guard denies, the two bootstrap stacks, GitHub OIDC, ECR repositories, budget alarms.

**Phase 0 also carries everything that cannot be retrofitted**, which is the only reason any of it is this early:

- **Cost attribution** (§16.1): activate the cost allocation tags, enable Split Cost Allocation Data for EKS, start CUR delivery. None of it backfills.
- **Audit trails** (§8.6): CloudTrail with log file validation, delivered to the compliance-mode Object-Locked bucket at 400-day retention. AWS Config with a narrow recording scope. A log is only tamper-evident from the moment validation is switched on, and retention cannot reach backwards.
- **Branch protection on `main`** with required review, which is both the CC8 change-management control and the thing that makes §8.6's "access review is a git diff" claim true.
- **Account floors that are one-liners now and findings later** (§5.6): account-level S3 Block Public Access in all three accounts, MFA on every root user, no root access keys.

Everything else in this document can be added later. These cannot, which is why they come before there is anything to observe. All of it is verifiable with no cluster running.

**Phase 1 — dev.** Network, EKS dev, Karpenter with the §5.1 disruption settings and the §5.6 node hardening, the nonprod lake bucket with lifecycle, EFS with automatic backups, External Secrets Operator with a reloader. Both Dockerfiles and the CI push job. Ship `ERSERVER_TENANT_DB_PREFIX`. The edge and email substrate lands here too, because both carry lead time or block Phase 2: the Cloudflare DNS zone wiring (§18.8), wildcard dev certificate, external-dns (Cloudflare provider), the internal ALB and the Tailscale subnet router (§4.1); the SES module with the production-access request filed (§7.5). This is also where the §5.5 Auto Mode check happens — before the Karpenter configuration deepens, not after.

**Phase 1 also brings the observability floor** (§10.6): the collector gateway and log-collection DaemonSet, structured logs shipped with trace fields, and FastAPI/psycopg auto-instrumentation. No schema change, and it means Phase 2 is debuggable rather than guesswork.

**Phase 2 — Per-developer environments, and the Job launcher.** The `er-dev-namespace` chart including its Postgres StatefulSet, the erweb migration hook Job, and the shared-ALB Ingress annotations (§12); `make dev-up` / `suspend` / `resume` / `down`, ResourceQuota with ephemeral-storage, the runner NodePools with NVMe, and the idle-suspend CronJob with the §5.2 predicate.

This is also where the runner becomes a Kubernetes Job (§6), which means it carries the coupled set: progress written by the runner (§6.3), `jobs.trace_context` and per-stage spans (§10.2), `force_flush()` on exit, per-run CPU sizing (§6.5), and the EFS mounts on the Job and the dispatcher (§6.2). Those are one piece of work, not six — they all sit on the boundary the subprocess pipe used to cover.

**Phase 3 — Prod, when there is prod traffic.** Network, EKS prod, Aurora prod multi-AZ, the prod lake with CMK and scoped CloudTrail, the prefix-scoped export role (§8.4), `make refresh-dev` Path A, and the internet-facing prod ALB behind WAF — rate rules on the three unauthenticated write paths (§4.1). Nothing here is needed to develop against real data, which is why it is sequenced last rather than second.

**Phase 4 — Hardening and the audit run-up.** The IRSA credential chain (§9.2) and the SES API port that retires the SMTP key (§9.3) — the two retirements §8.1 now promises — per-tenant scoped ServiceAccounts (§6.4), `ER_PROFILE_*` output as span events, dashboards for the [backend-design.md](backend-design.md) §14 metric list, alerting, automated snapshot sharing. GuardDuty in prod. The SOC 2 artifacts that are point-in-time rather than time-dependent — policies, risk assessment, the subprocessor inventory including the Anthropic data flow, vendor assessments, a documented break-glass procedure, and a restore test with its record (§8.6).

**A follow-up worth noting:** an EKS cluster unlocks the GitHub Actions Runner Controller, which resolves a recorded blocker — [benchmarks/scales.yaml](../benchmarks/scales.yaml) asks for `ubuntu-latest-8-cores` and `ubuntu-latest-16-cores` at the `100k` and `1m` scales, neither available to this repository today. A `runner-l` node serves them natively.

## 16. Cost

Rough `us-east-1` monthly figures. Estimates for shaping decisions, not a quote.

The first draft put dev at ~$185. That was optimistic by roughly 2×: it assumed Aurora auto-pause that cannot happen (§7.1), and omitted interface endpoints that cost more than the NAT they replaced (§4). Corrected, and with §5.2's suspend-on-idle:

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

The runner line dropped despite more vCPU: §6.5's switch from memory-optimized `r6id` to compute-optimized `c6id` buys the same 16 vCPU and NVMe for about a third less, because 96 GiB of the original RAM was never going to be used.

Because prod is deferred (§15), the real near-term number is the dev column: **roughly $190/mo**, and genuinely idle-cheap rather than nominally so.

The fixed floor is $73 control plane + $24 system nodes + $33 NAT + $16 ALB + $4 Tailscale = **$150**, which is 78% of it and none of it elastic. The remaining ~$42 is what suspend-on-idle actually moves. Worth being clear-eyed about: the savings mechanisms in §5 and §7.1 address the small half of the bill. If $180/mo matters, the lever is not more autoscaling — it is dropping the ALB for `kubectl port-forward` (−$16), accepting a public-subnet node group with no NAT (−$33), or questioning whether a managed control plane is needed for one developer at all.

Four line items are load-bearing assumptions rather than quotes: spot pricing on the runner pools (safe only because `dispose()` treats a killed runner as resumable), CloudTrail data events (§18.4), observability log volume — dbt alone can emit hundreds of megabytes on a 10M-row run at ~$0.50/GB ingested, which is why §10.4 makes log level environment-configured rather than a default — and lake storage, which scales with how much prod data dev actually holds and is not estimable until a refresh has run.

Note that observability is the one line here that grows with *usage* rather than with provisioning. At one developer it is noise; at a hundred tenants running nightly it can become a top-three item, and the mitigations (sampling, `tenant` kept off metric series per §10.3, retention) are all much cheaper to design in now than to retrofit once dashboards depend on them.

Budget alarms at $300 and $600 are part of Phase 0.

### 16.1 Measuring actual spend, not just estimating it

Everything above is an estimate. Three mechanisms turn it into measurement, and the first one has a sequencing trap.

**Cost allocation tags.** Every resource carries `Project=er`, `Environment`, `ManagedBy`, and — on per-developer resources — `Developer`, via the provider's `default_tags`. These must be **activated as cost allocation tags in the Billing console, in Phase 0, before anything is created**: activation takes up to 24 hours and *does not backfill*. Activate them late and the first weeks of spend are permanently unattributable. This is the cheapest thing in this document to get right and the most annoying to discover afterwards.

Tags answer the dev-versus-prod question directly, because the account split already separates them — Cost Explorer grouped by linked account is the breakdown, and `Environment` is the cross-check.

**Cost and Usage Report → Athena.** Cost Explorer is fine for eyeballing. Anything programmatic — a per-tenant monthly figure, a cost-per-million-records trend — needs the CUR delivered to S3 and queried with Athena. Partition it; an unpartitioned CUR makes Athena scan the whole history on every query and the scanning becomes its own line item.

**Split Cost Allocation Data for EKS.** This is the mechanism that makes the rest possible, and it is easy to miss. A Kubernetes pod is not an AWS resource, so ordinary tags cannot see it — tagging gets you node-level cost, not pod-level, which is useless for per-tenant attribution when many tenants' Jobs share a node. Enabling split cost allocation adds per-pod rows to the CUR, dividing each instance's cost across the pods that ran on it by CPU and memory request versus usage, with namespace and workload surfaced as columns.

Two payoffs fall straight out of design choices already made for other reasons:

- **Per-developer cost comes free**, because §12 makes a developer's environment a namespace. Group the CUR's split rows by namespace and you have what Alice's environment cost last month.
- **Per-tenant compute cost is tractable**, because §6 makes a run a dedicated single-tenant pod. The hard part of cost allocation is shared processes; the runner has none.

### 16.2 Per-tenant attribution: direct costs and an allocation basis

Be honest about the shape: some costs are directly attributable and some are shared, and pretending otherwise produces a number nobody trusts. The good news is that the shared portion is mostly *fixed*, and the variable portion is mostly *direct*.

| Cost | Attribution | Basis |
|---|---|---|
| **Runner pods** — the dominant variable cost | **Direct** | One Job, one tenant, for its whole life. CUR split rows by pod, or the internal model below |
| **Lake storage** | **Direct** | Per-tenant prefix (§7.3). S3 Inventory → Athena gives exact bytes per prefix; Storage Lens gives it approximately with no setup |
| **Catalog storage** | **Direct** | `pg_database_size()` per tenant database — a dedicated database per tenant (§7.1) makes this a one-line query |
| **Lake request costs** | Direct but expensive to collect | Requires S3 server access logs or CloudTrail data events grouped by prefix (§18.4) |
| `er-api`, `er-web`, `er-dispatcher` pods | **Shared — not attributable**, as you noted | Allocate by share of runs, or leave as platform overhead |
| Aurora compute | **Shared** | Allocate by catalog size or run count; the cluster serves every tenant |
| EKS control plane, NAT, ALB, system nodes, EFS | **Fixed overhead** | Do not allocate. §16's floor is $146 of this, and spreading it across tenants makes small tenants look falsely expensive |

**Also compute per-run cost internally, in Postgres.** Not instead of the CUR — alongside it, for a different purpose. Every term is already recorded or known: `run_stages` has per-stage duration, §6.5 sets the pod's vCPU and memory, and the node's hourly price follows from its instance type. Cost per run is their product, and writing it onto the run row makes it queryable *next to* the business counters rather than in a separate tool.

The trade is deliberate: this is allocated cost at a blended rate, not billed cost, so it will not reconcile to the penny — spot prices move, and the fixed overhead above is excluded. A figure within ~10% that you can join to row counts in SQL is more useful for answering "what is this tenant costing us" than an exact figure in a separate console. The CUR stays the authority for the actual bill; reconcile the two monthly and investigate drift rather than treating either as wrong.

**Kubecost and OpenCost are the obvious tools here and are not recommended initially.** OpenCost is the right shape — per-namespace and per-label pod cost — but it wants Prometheus, which §10.5 deliberately does not run, so adopting it means standing up a metrics store for one consumer. Split cost allocation now covers the same ground natively. Revisit if real-time cost visibility becomes worth that, or if per-label (rather than per-namespace) granularity turns out not to be available (§18.11).

### 16.3 Knowing when a tenant's cost means something is wrong

This is not a pricing model. The purpose is narrower and more useful: **a signal that tells you when the current design has stopped working for some customer shape, early enough to do something about it.** What to charge is a separate question that does not need infrastructure to answer.

The business half needs no new instrumentation. §10.1 establishes `runs`/`run_stages` as the system of record for business counters, and the typed set in `PROMOTED_COUNTERS` ([src/er/obs/counters.py](../src/er/obs/counters.py), [src/er/lake/model.py](../src/er/lake/model.py)) is already written per stage per run — the 10M-row reload recorded 3,898,183 golden records, 23,389,098 lineage rows, and 567,184,685 candidate pairs with nothing added ([measurements/workload-10m-100k-20260923.json](measurements/workload-10m-100k-20260923.json)). Joined to §16.2's per-run cost, that is three questions and one SQL query.

**1. Is any tenant disproportionate?** Cost per tenant per month, ranked. Normalize by records processed — **cost per million records** — so a large tenant and a small one are comparable. A big tenant costing more is not a signal; a *small* tenant costing like a big one is.

**2. Is cost per million stable, rising, or falling?** Month over month, per tenant and in aggregate. This is the degradation detector and the most valuable of the three, because it catches problems while they are still cheap.

**3. How much is being spent on runs that failed?** A run that dies in the last stage costs nearly what a successful one does, and a spot eviction retried three times under `max_attempts` costs three times (§5.3). This is invisible in any per-successful-run figure.

#### What each signal would actually mean

The point of the signal is the action it implies, so:

| Signal | Likely cause | The pivot it points at |
|---|---|---|
| One tenant is a large share of variable spend | Their corpus or data shape does not fit a shared cluster | A dedicated node pool, or the enterprise tier [backend-design.md](backend-design.md) §7 already earmarks for dedicated buckets and STS credentials. Check their config first — loose blocking rules inflate candidate pairs superlinearly, and 567M pairs on a 10M corpus is what drives the cost |
| Cost per million rising for **one** tenant | Their data got messier; blocking keys degrading | A config problem before an infrastructure one. Tune blocking, not instance types |
| Cost per million rising **across all** tenants | Systemic | Snapshot retention not expiring (§7.3's lifecycle, `er lake maintain`), or a correction-pass cadence running more often than the data changes |
| Failed-run spend is a meaningful share | Spot interruptions or §5.3 evictions | Move the L class off spot, or fix the ephemeral-storage sizing. Both are cheap; neither is obvious without the number |
| Fixed overhead still dominates everything | You do not have a tenant cost problem yet | Look at §16's $146 floor instead — ALB, NAT, control plane |

That last row matters more than it looks. At one developer the fixed floor *is* the bill, so per-tenant numbers are noise until variable spend is a real fraction of the total. The first thing this reporting should tell you is which regime you are in, so effort goes to the half that is actually costing money.

#### Scope

Deliberately small: one Athena query over the CUR, one SQL query over `runs`/`run_stages`, run monthly. Two alarms worth automating — any single tenant exceeding a set share of variable spend, and failed-run spend exceeding a set share of total — because those are the two that are cheap to detect and expensive to notice late. Everything else is a query you run when a number looks odd, not a dashboard that needs maintaining.

## 17. Verification

Each phase has a behavioural check, not a green `terraform apply`. Several of these exist specifically because the first draft asserted things it had not tested.

**Phase 0 — the security claim.** The assertion §8.2 exists to make, probed against the guard denies the permission sets carry (rev 4: IAM-layer, so the probe asserts the deny itself, not an SCP). The prod lake does not exist until Phase 3, so Phase 0 asserts against the prod-prefixed names directly:

```bash
aws sso login --profile platform-admin
aws s3api list-objects-v2 --bucket er-prod-lake      # read path intact (NoSuchBucket until Phase 3 is fine)
aws s3api put-object --bucket er-prod-lake --key probe   # MUST fail: AccessDenied, Statement 1 (§8.2)
aws iam create-role --role-name er-prod-app-probe \
  --assume-role-policy-document file://trust.json    # MUST fail: Statement 2 (§8.2)
aws sso login --profile prod-admin
aws iam create-role --role-name er-prod-app-probe ... && aws iam delete-role --role-name er-prod-app-probe   # succeeds — the exemption works
```

The second and third commands failing under `PlatformAdmin` is the point. The third specifically tests the privilege-escalation statement — if it succeeds, the `er-prod-*` exemption is an open door. Re-run the full form against the real lake once Phase 3 lands, and re-run it **after any permission-set change**, because rev 4 moved the guard into exactly the layer such a change edits.

**Phase 0 — the things that cannot be checked later.** Both are verified now precisely because neither backfills.

Cost attribution (§16.1): confirm the cost allocation tags show as *Active* in Billing and that a CUR file has landed with split cost allocation columns present. A cluster that runs three weeks before anyone looks produces three weeks of permanently unattributable spend.

Audit trails (§8.6): confirm CloudTrail digest files are being written and `aws cloudtrail validate-logs` passes; confirm the archive bucket's retention is 400 days and Object Lock is in compliance mode; then, as the control test rather than an inspection, **assume `PlatformAdmin` in nonprod and attempt to delete an object from the archive bucket — it must fail.** That is the property an auditor is actually checking: that the principals a log records cannot erase it.

**Phase 1 — image and registry.** Push from CI; confirm tag immutability rejects re-pushing an existing tag, and that the digest replicated into prod matches nonprod exactly. Confirm a PR build cannot assume the push role.

**Phase 2 — a developer environment, and nine things likely to be silently wrong.**

```bash
make dev-up DEV=alice
# log in at alice.dev.<domain>, create an org, submit run_all_full
kubectl -n alice get jobs -w
kubectl -n alice exec deploy/er-api -- df -h /app/.tmp   # NVMe, not the root filesystem
```

- **The parameter actually took effect.** Run `er lake maintain`, which is precisely the operation that fails at the default of 64 locks.
- **Suspend really reaches zero.** `make dev-suspend DEV=alice`, then confirm Karpenter reclaims the `app` node and no pod holds a database connection. This is the §5.2 claim, and the first draft asserted its equivalent without testing it.
- **The catalog survives suspension.** Suspend, resume, and confirm golden-record counts and snapshot history are unchanged. §7.1 rests on the PVC outliving the pod; a misconfigured StatefulSet volumeClaimTemplate silently gives an `emptyDir` instead.
- **NetworkPolicy enforces.** From Alice's namespace, attempt to reach Bob's Postgres service. It must fail. Under the VPC CNI without the policy agent enabled, this *succeeds* while the NetworkPolicy object looks correct (§8.5).
- **Kubernetes does not retry the Job.** Submit a run with a deliberately invalid config (engine exit 2, which `dispose()` must fail permanently) and confirm exactly one pod is created. A default `backoffLimit` of 6 would re-run it six times, and on a *partially completed* pipeline it would do so without `--resume` (§6.2).
- **One trace covers the whole run.** Submit from the UI and confirm a single trace spans BFF → API → queue wait → Job pod → every stage → dbt. Two disjoint traces joined only by `run_id` means `jobs.trace_context` is not being propagated (§10.2). Then check the *final* stage and the exit status are present — a missing tail is the `force_flush()` gotcha, and it is invisible unless you look for it specifically.
- **Logs carry no attribute values.** Grep a completed run's logs for a known name, email, and phone number from the seeded fixture. All three must be absent. This is the rule §8.5's prod log-read grant depends on, so it is verified rather than assumed, and it is worth re-running whenever a stage gains new logging.
- **The edge is actually internal.** From a network without Tailscale, `alice.dev.<domain>` must not connect; over the tailnet it must. And `kubectl get ingress -A` must list `er-web` only — er-api on an Ingress anywhere means the §4.1 boundary is fiction.
- **IMDS is unreachable from pods.** `kubectl exec` into an app pod and curl `169.254.169.254` with a one-second timeout — it must time out. hostNetwork system pods still reach it; that split is the design (§5.6).

**Phase 3 — refresh and spot resilience.** Run `make refresh-dev` and diff golden-record counts against prod. Then terminate a runner node mid-job and confirm the job requeues with `--resume` and completes — this validates the assumption §16 prices spot against. Separately, confirm a spill-heavy run is *not* evicted, which is the §5.3 sizing.

**The existing test tiers remain the gate.** `make check-all` must pass unchanged. Two contract tests sit in the blast radius of the image work: `tests/unit/test_dockerfile_contract.py` fails if the runner image drops its dev dependencies (the compose `pipeline` service's command *is* pytest), and `tests/unit/test_dbt_profiles.py` holds the three S3 configuration sites field-for-field identical. The integration tier, not the fast tiers, is what catches cross-module regressions — run the full pass before merging Dockerfile changes.

## 18. Open items

Measurements and decisions this design is waiting on. The first four are things the first draft assumed.

1. **Is the one-Postgres-instance constraint real?** §2 cites [backend-design.md](backend-design.md) §5 for the queue and the tenant advisory lock needing one cluster. PostgreSQL advisory locks are scoped per *database*, and the lock is taken on the tenant catalog DB while the queue lives in the control-plane DB — so they are in different databases regardless, and the serialization guarantee appears to come from the `jobs_one_active_per_org` unique index instead. If the constraint is over-stated it removes a real design option; §7.1 satisfies it either way, so this is not blocking.
2. **Where the thread-scaling curve flattens.** §6.5 makes vCPU the sizing lever on the strength of a 2→6 thread measurement that cut 10M-row runtime 40%. The measurement stops at 6, and it is unlikely to stay linear: the same run showed **reconcile 14% *slower*** at 6 threads, being a Python graph step that does not parallelize with DuckDB threads. Past some count the Python stages dominate and further vCPU buys nothing. A 10M run at 6 / 12 / 16 / 24 threads sets the top of the class ladder, and the same run with `ER_PROFILE_STORAGE` confirms spill at the 8 GB limit the L class actually uses — which is near the 4 GB the 141 GiB figure was measured at, so this is a confirmation rather than an extrapolation.
3. **~~Does EKS Auto Mode support `instanceStorePolicy: RAID0`?~~ Resolved — yes, automatically.** Auto Mode formats instance-store NVMe for ephemeral use and stripes RAID0 across multiple drives with no field to set. The residue that now decides §5.5: whether Auto Mode's NodePool surface honours `do-not-disrupt` and a `WhenEmpty` equivalent — the §5.1 controls that keep a bin-packing consolidation from discarding an hour of a run — plus the surcharge arithmetic at this footprint.
4. **Telemetry volume per pipeline run** — CloudTrail data events, and separately OTel log/span volume. Decides whether §7.3's scoped trail is affordable, whether §10.4's log levels are aggressive enough, and whether traces need tail sampling. Both are answerable from one instrumented 10M run, and both are §16 line items currently carrying a range instead of a number.
5. **Whether the dispatcher should watch pod logs or the runner should write its own progress** (§6.3). The recommendation is the latter, but it is a behavioural change to a path the frontend's job-detail view depends on, so it wants a decision before the Job launcher is written rather than during.
6. **DR posture.** Single region, no cross-region replication, no stated RPO/RTO for customer CRM data. Aurora PITR and S3 versioning cover deletion and corruption, not regional loss. Fine for now; it should be a stated accepted risk rather than an omission.
7. **DuckLake path storage** — absolute or relative in the catalog. Determines the size of `er lake rehome` (§13) and therefore whether Path A is permanent.
8. ~~**Hosted zone and domain**~~ **Resolved 2026-10-10: `dupezero.com`, registered at Cloudflare.** Cloudflare Registrar pins the apex to Cloudflare's nameservers, so **Cloudflare is the DNS plane** — no Route 53 zone. Consequences, all mechanical: external-dns runs its Cloudflare provider (scoped API token from Secrets Manager, `--domain-filter=dev.dupezero.com`, `--txt-owner-id` per cluster) and must create **DNS-only (unproxied) records**, since dev hostnames resolve to an internal, tailnet-reached ALB that Cloudflare's proxy could never reach; ACM stays the certificate authority (the wildcard dev cert validates via a CNAME the Terraform `cloudflare` provider writes); SES Easy-DKIM CNAMEs and MAIL FROM records land in the Cloudflare zone the same way.
9. **Resource classes need revising in [backend-design.md](backend-design.md) §7, not just implementing.** That document specifies S 8 GiB / M 24 GiB / **L 64 GiB**, and §6.5 here shows the measurements do not support it — 10M records peaked at 9.37 GiB, and runtime is CPU-bound. `resource_class` appears nowhere in `server/src` or `frontend/src` yet, so nothing is built on the old numbers; this is a chance to correct the spec before it is implemented rather than after. The revised shape should land in `backend-design.md` so the two documents do not disagree.
10. **API pod DuckDB sizing** — until the [backend-design.md](backend-design.md) §8 split is implemented, a tenant configured for a large runner also sizes the API pods' attaches (§2). The `app` NodePool is sized defensively as a result.
11. **What granularity Split Cost Allocation Data actually exposes.** §16.1 and §16.2 assume per-pod CUR rows carrying namespace and workload, which covers per-developer (namespace) and per-tenant-run (workload) cleanly. Whether arbitrary pod *labels* surface as CUR columns decides whether per-tenant grouping is a direct query or needs joining through the `jobs` table on workload name. Either works; the second is more code. Worth confirming against one real CUR before building reporting on top of it.
12. **Whether the internal cost model and the CUR reconcile.** §16.2 deliberately keeps two numbers — allocated at a blended rate in Postgres, billed in the CUR — and claims they should agree within ~10%. That tolerance is a guess until a month of both exists. If the gap is much wider the likely causes are spot price variance and pods that request far more than they use, and the second is actionable (§6.5's sizing is only as good as its requests).
13. **Which Trust Services Criteria the first audit is scoped to.** Security is mandatory. **Confidentiality** is the natural second given the data, and it is largely covered by §8.3's encryption and §8.1's access control. **Availability** is the expensive one to add, because it brings a documented RPO/RTO and *evidence of a restore test* — a backup never restored is a finding, so §18.6's deferred DR posture becomes required work the moment Availability is in scope. Scoping to Security + Confidentiality first is the lighter path and a decision worth taking deliberately rather than by default.
14. **The Anthropic data flow needs a documented position**, not just an inventory entry (§8.6). The assistant sends names, emails, and full golden records to a third party. Minimum: a DPA, a configured retention posture on the vendor side, customer disclosure, and a decision on whether tenants can opt out. This is product work with a compliance deadline attached, and it is the item on this list most likely to be discovered by someone other than us.
15. **Does `sslmode=verify-full` work with the Aurora CA bundle as the images are built?** Partially answered by inspection: the Dockerfile installs nothing — no `apt-get`, no `ca-certificates`, no RDS bundle — so the runtime has only what `python:3.12-slim` ships. Expect an image change (install `ca-certificates` plus the RDS global bundle), not a secret change; what stays open is only verifying the bundle path wiring in each DSN.
16. **Cold-node image pull time** for the current `er-pipeline`. Decides how far down the §11.1 ladder to climb — SOCI alone, the image split, or snapshot prebake — and nothing measures it today.
17. **Whether Phase 4's two credential retirements land as one change or two.** The S3 credential chain (§9.2) and the SES API port (§9.3) touch disjoint code but retire the same §8.1 amendment; doing them together makes "no static keys anywhere" a single verifiable claim rather than two partial ones.
