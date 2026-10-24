# Infrastructure design decisions

*The decision log for the AWS/EKS/Terraform design in [infrastructure.md](infrastructure.md). That document describes the **current-state** design; this one holds the **why** — the revision history, every "the first draft said X; corrected to Y" decision, the rejected alternatives, and the open items that have since been resolved. Reasoning is preserved here verbatim where it was moved out of the design doc. Product overview: [../README.md](../README.md). Live build status: [plan-to-aws.md](plan-to-aws.md).*

Entries are ADR-style: **Decision**, **Context**, **Rejected alternative**, **Date**. Section references (§N) point into [infrastructure.md](infrastructure.md).

---

## Revision history

**Revision 1 — 2026-10-02 (original draft).** The first AWS footprint: a three-account organization (`mgmt`/`nonprod`/`prod`), one EKS cluster per environment, a namespace-per-developer dev environment, and a security model where prod is readable by the team but writable only from an explicitly assumed role. It asserted three cost savings it had not substantiated, specified interface endpoints, chose memory-optimized `r6id` nodes with memory-tiered resource classes, and put dev at ~$185/mo.

**Revision 2 — 2026-10-03.** Two rounds of changes. First, the original draft asserted three cost savings it had not substantiated — Karpenter "scale to zero", Aurora auto-pause, and interface endpoints as a NAT-avoidance measure. All three were wrong or incomplete and the dev estimate was optimistic by roughly 2×; §5.2, §7.1, and §16 were rewritten. Second, §6 (job execution) and §10 (observability) were new, and §6.5 revised run sizing from memory-tiered to CPU-tiered on the strength of measurements already in the repo — which also changed the runner instance family from `r6id` to `c6id`. §18 carried what the design was still waiting on.

**Revision 3 — 2026-10-03 (third pass).** A cross-examination against the repository and current AWS behaviour. What changed: the §6.2 Job spec omitted the EFS mount the runner cannot run without — and the dispatcher turns out to read org configs off that filesystem on every idle tick, so it mounts it too; dev environments holding real records were implicitly internet-facing behind nothing but the application login, and §4.1 now puts them behind an internal ALB reached over Tailscale; email was treated as a future feature when it is a built, wired subsystem (§7.5), and SES SMTP structurally cannot use temporary credentials, which adds a second static key to §8.1's ledger until Phase 4 (§9.3); §4's "let the node cache do the work" was defeated by the runner pools' own scale-to-zero (§11.1); single-instance-type spot pools were the worst shape to buy spot in (§5); §14.2 now pins who may run Terraform and deploy, because the §8.2 SCPs exempt a role this document had never named; and §18.3's Auto Mode question is answered — NVMe RAID0 is automatic — which reopens §5.5. New sections: §4.1, §5.6, §7.5, §9.3, §11.1, §14.2.

**Revision 4 — 2026-10-10 (single account).** The three-account organization collapsed to a single AWS account in `us-east-2`. See **D-ACCT** below for the full account-collapse story; the region choice is **D-REGION**. The three-account design remains in git history (and in revisions 1–3 of this log's narrative) as the shape to return to when the team grows.

---

## D-ACCT — Collapse to a single AWS account

**Decision.** One account (operator decision). Environments are separated by `er-dev-*` / `er-prod-*` prefix (buckets, roles, clusters, secrets) and by Identity Center permission-set policy, not by an account boundary. Minimal operational surface: one login, one bill, one root user, no cross-account plumbing.

**Context.** Revisions 1–3 specified a three-account organization (`mgmt`/`nonprod`/`prod`). The operator chose to collapse to a single account to keep the operational surface minimal. What that trades away — recorded deliberately, accepted by the operator:

- **SCPs no longer apply to anything.** Service control policies never evaluate against the organization's management account, which is now the only account. Every §8.2 guardrail is therefore an IAM-layer deny carried by the permission sets — enforceable, but one policy edit from absent, where the OU boundary was structural. This was the prior revision's central argument for three accounts.
- **Blast radius is shared.** Dev mutation is unrestricted by design (§8.1), and dev mistakes now share an account — and its service quotas — with prod data.
- **The audit archive lives with the workloads.** The compensating control is S3 Object Lock in compliance mode: 400-day retention that not even the root user can shorten, preserving tamper evidence without an account boundary (§8.6).
- **SOC 2 CC6 is evidenced from policy text** rather than account membership.

The former `nonprod` member account (660360495170) is parked empty at the organization root at zero cost; close it or re-adopt it if the split returns. The organization root is kept because Identity Center requires it; otherwise it is inert. The account id is `er` (797781631727).

Consequences that rippled through the design: the §8.2 guard statements moved from an SCP on the prod OU to inline denies on the permission sets; the §11 registry's nonprod-source → prod-replica replication disappeared (promotion is by digest and never depended on replication); the §14 Terraform-state trade-off (prod state living in the nonprod account) dissolved into the broader "protected by role policy, not an account boundary" position; and the §8.4 dev→prod read chain kept its two-role shape (so the readable surface is one role's policy) even though both roles are now same-account.

**Rejected alternative.** The three-account organization (`mgmt`/`nonprod`/`prod`) of revisions 1–3. Its central argument was that structural separation (an OU boundary enforced by SCPs) beats policy correctness — an SCP sits above the identity-policy layer and catches a wrongly written IAM policy, where an inline deny *is* the identity-policy layer and whoever can edit permission sets can remove the guard. The operator accepted that weaker posture in exchange for minimal operational surface, with the Object-Locked audit archive and `main` branch protection as compensating controls. The three-account design is the first thing to reinstate if the team grows; a dedicated log-archive account is the first piece of it to bring back.

**Date.** 2026-10-10.

---

## D-REGION — `us-east-2`, single region, no DR

**Decision.** `us-east-2`, single region, no DR replication in this phase.

**Context.** Chosen when the Identity Center organization instance was created there — its primary region is permanent. Every service this design uses is available in us-east-2 at the same price points; the repo's `ER_S3_REGION` sites ([docker/compose.yaml](../docker/compose.yaml), [frontend/dev/env.sh](../frontend/dev/env.sh), [dbt/profiles/profiles.yml](../dbt/profiles/profiles.yml)) match.

**Rejected alternative.** Multi-region and a real DR posture — deferred (§18.6 carries it as a stated accepted risk rather than an omission).

**Date.** 2026-10-10.

---

## D-ARGO — A native Kubernetes Job per run, not Argo Workflows

**Decision.** A native Kubernetes Job per run, behind the `launch` seam that already exists — not Argo Workflows.

**Context.** There is real prior art for multi-tenant ephemeral data pipelines on Argo Workflows, and the instinct is sound. It is the wrong fit here for a specific reason: **the DAG is inside the process, not across pods.** `run_all_full` executes ingest → standardize → train → match → reconcile → assemble as library calls in a single process via [src/er/service.py](../src/er/service.py), with dbt invoked as a subprocess inside that same pod. There is no artifact to pass between pods, no step to fan out, and no dependency graph for Kubernetes to resolve. Argo's core value — orchestrating multi-step DAGs across pods with artifact passing — would have nothing to orchestrate.

What it would add is a **second, competing copy of control logic that already exists and is tested**:

- **Retry semantics.** [server/src/erserver/policy.py](../server/src/erserver/policy.py) keys retries to the engine's exit-code taxonomy: `0` succeeded, `10` no-op, `2` fails permanently (config error — must never retry), `3` + `lock_conflict` requeues after 30s *without consuming an attempt*, `1` + `transient_io` or a `None` exit requeues *with `--resume`* under exponential backoff. Argo's `retryStrategy` does not know any of that, and expressing it there would put the policy in two places.
- **Two sources of truth for run state.** The `jobs` table is what the API's `/runs` endpoints and the frontend's job detail view read, with live per-stage progress in `jobs.progress`. An Argo workflow object would shadow it, and the two would disagree the first time one of them was restarted.
- **Two schedulers.** Cron already lives in the dispatcher's leader loop via `croniter` ([schedules.py](../server/src/erserver/schedules.py)). Argo CronWorkflows would be a second thing able to start a tenant's run, and it *cannot* honour the `jobs_one_active_per_org` partial unique index, because that guarantee is enforced by the dispatcher's `FOR UPDATE SKIP LOCKED` claim query. Two dispatchers means the single-writer-per-tenant property is gone.

[backend-design.md](backend-design.md) §5 already rejected Celery and Redis on structurally identical grounds — "at-least-once redelivery is actively wrong for an engine that deliberately never auto-retries." Argo is a different tool with the same mismatch: an orchestrator with its own opinions about retries and state, layered over an engine whose state machine is already complete and deliberate. There is also a roadmap cost: [backend-design.md](backend-design.md) §15 phase 5 contemplates a self-hosted option, and the existing non-k8s fallback (the dispatcher forking a subprocess) serves that today. Argo would become a hard dependency for self-hosters.

Argo **CD** is a separate question. GitOps for deploying the platform itself is defensible and does not touch the job path; it is simply out of scope.

**Rejected alternative.** Argo Workflows for the job path.

**Revisit when.** A run ever becomes a genuine multi-pod DAG — sharding match across pods, which the one-writer-per-tenant constraint currently forbids — or if workflow visualisation for non-engineers becomes a product requirement. Neither is true now.

**Date.** 2026-10-03.

---

## D-ENDPOINTS — No VPC interface endpoints

**Decision.** Interface endpoints are deliberately not used. Non-S3 traffic goes through the NAT. (The S3 Gateway Endpoint is mandatory and free — it is kept; this decision is about *interface* endpoints only.)

**Context.** The first draft specified them for ECR (api + dkr), Secrets Manager, STS, and CloudWatch Logs, justified as keeping node bootstrap off the NAT path. That reasoning ignored their price: five endpoints × three AZs × ~$0.01/hr is **~$110/mo per environment**, against the $33 NAT they were avoiding. For a one-developer footprint this inverts the decision. The only non-S3 volume that looked material was ECR image pulls — but it is not: ECR serves image *layer blobs* from S3 (the regional `prod-us-east-2-starport-layer-bucket`), so the bytes of a pull ride the free gateway endpoint and only the manifest and auth calls cross the NAT. What a cold node's pull costs is time, not money (§11.1).

**Rejected alternative.** Interface endpoints for ECR/Secrets Manager/STS/CloudWatch Logs.

**Revisit when.** There are enough nodes that per-GB NAT processing exceeds the fixed endpoint cost, or a compliance requirement forbids the NAT path.

**Date.** 2026-10-03.

---

## D-IDLE — Namespace-suspend, not per-resource autoscaling

**Decision.** Suspend the namespace to drive idle cost to near-zero — one control that takes node count, database, and compute to zero together — rather than three independently tuned autoscaling mechanisms.

**Context.** Karpenter removes a node when its pods are gone. It cannot remove a node whose pods are still scheduled. A developer namespace running `er-api`, `er-dispatcher`, and `er-web` around the clock keeps an `app` node alive permanently, and **no Karpenter setting changes that.** The first draft's "scale to zero" claim was about the runner pools only, and read as though it covered the environment; it listed what are really one control (suspend) as three independent savings. Suspending the namespace cascades: with the pods gone the `app` node empties and Karpenter reclaims it; the dispatcher's permanent leader connection goes away, so nothing holds the database open; and the Postgres pod stops billing compute while its EBS volume persists. The suspend predicate had to be written carefully because the obvious signal does not exist — there is no request log anywhere in the system. What exists is `erweb.sessions.last_seen_at`, slid on activity but throttled to at most hourly ([frontend/src/lib/auth/session.ts:13](../frontend/src/lib/auth/session.ts)): coarse, and sufficient for a four-hour threshold.

**Rejected alternative.** Per-resource autoscaling (Karpenter "scale to zero" as an environment-level cost strategy), treated as three separate savings.

**Date.** 2026-10-03.

---

## D-AUTOPAUSE — A Postgres pod in dev, not Aurora Serverless v2 auto-pause

**Decision.** `postgres:16` StatefulSet per dev namespace; Aurora PostgreSQL 16 Serverless v2 in prod only. Not Aurora auto-pause in dev.

**Context.** The first draft specified Aurora Serverless v2 with auto-pause in dev. But auto-pause requires *no active connections*, and §2 records that the dispatcher holds a permanent leader connection polling every two seconds. The cluster would never pause while any namespace was up, making the floor 0.5 ACU — about **$44/mo**, not the ~$5 the first draft's cost table assumed. The saving was illusory. A pod that scales to zero with its namespace (§5.2) delivers what auto-pause was supposed to. A pod is also *closer* to tested parity than Aurora, which is counterintuitive but true: both compose files run `postgres:16` with `-c max_locks_per_transaction=1024`, so a pod matches what the integration tier exercises and Aurora is the configuration that diverges. What dev gives up, all acceptable because prod is the system of record: PITR, managed backups, Multi-AZ, and cross-account snapshot sharing for the refresh path — dev uses `pg_dump` instead (§13).

**Rejected alternative.** Aurora Serverless v2 with auto-pause in dev.

**Date.** 2026-10-03.

---

## D-COST2X — The ~2× dev cost correction

**Decision.** Dev is ~$190/mo, not the ~$185 the first draft put it at — and the first draft was optimistic by roughly 2× before the two offsetting corrections.

**Context.** The first draft put dev at ~$185. That was optimistic by roughly 2×: it assumed Aurora auto-pause that cannot happen (D-AUTOPAUSE / §7.1), and omitted interface endpoints that cost more than the NAT they replaced (D-ENDPOINTS / §4). Corrected, and with §5.2's suspend-on-idle, the dev subtotal is ~$192, of which a $150 fixed floor (control plane + system nodes + NAT + ALB + Tailscale) is 78% and none of it elastic. The savings mechanisms in §5 and §7.1 address the small half of the bill.

**Date.** 2026-10-03.

---

## D-NODE — Compute-optimized `c6id`, not memory-optimized `r6id`

**Decision.** Compute-optimized with local NVMe (`c6id` and siblings), not memory-optimized (`r6id`).

**Context.** The first draft chose `r6id.4xlarge` to host a 64 GiB pod, following [backend-design.md](backend-design.md) §7's resource classes. The measurements say that was wrong on both counts: a 10M-record reload peaked at **9.37 GiB** of container memory, and going from 2 to 6 DuckDB threads cut its runtime **40%** with memory essentially flat. Runtime is CPU-bound and memory is nearly constant in corpus size, because DuckDB spills rather than growing its working set (§6.5). So the right node has many vCPU, modest RAM, and a large NVMe for spill. `c6id.4xlarge` matches `r6id.4xlarge`'s 16 vCPU and 950 GB of NVMe while dropping 128 GiB of RAM to 32 GiB, for roughly **a third less per hour** — which is why the §16 runner line dropped despite more usable vCPU (96 GiB of the original RAM was never going to be used).

**Rejected alternative.** Memory-optimized `r6id.4xlarge` with a 64 GiB pod.

**Date.** 2026-10-03.

---

## D-SPOT — Many instance families and both capacity types, not single-instance-type spot

**Decision.** List every current x86 compute-shaped instance with local NVMe in each runner NodePool, and list `spot` and `on-demand` together.

**Context.** The second revision listed exactly one instance type per runner pool, which is the worst shape to buy spot in — one type in one size raises both the interruption rate and the odds of plain unavailability. Karpenter makes width free: list `spot` and `on-demand` together in one NodePool and it strictly prefers spot, falling back to on-demand automatically when spot capacity comes up empty — the documented fallback pattern, not a cost risk. The families are `c6id`, `c8id` (GA February 2026, same 474/950 GB NVMe shapes), `c5ad`, `c5d` — there is no `c7id`; Intel's seventh x86 generation skipped the NVMe variant — plus `m6id` as a deeper fallback that merely wastes some RAM. One exclusion is load-bearing: `c5d.4xlarge` carries only 400 GB of NVMe against the L class's 400 GiB spill limit — negative headroom before the image and kubelet overhead land — so it stays out of `runner-l`, while `c5d.2xlarge` (200 GB against S/M's 40–150 GiB) stays in `runner-sm`.

**Rejected alternative.** A single instance type per runner pool (the second revision's shape).

**Date.** 2026-10-03.

---

## D-CLUSTER — One cluster per environment, not one cluster total

**Decision.** One EKS cluster per environment; keep two, build only one now (dev first, prod deferred until there is prod traffic — §15).

**Context.** Collapsing dev and prod into one cluster with namespace separation saves roughly **$97/mo** — a $73 control plane and a second system node group — plus another $16 if prod gets its own ALB. What the second cluster buys:

- **Cluster-scoped blast radius.** A bad CRD, a webhook that fails closed, a CNI upgrade, a Karpenter version bump: all cluster-wide, none able to cross.
- **A staging-shaped upgrade path without a staging environment.** Dev goes to the next EKS minor first and reveals what breaks. With one cluster, dev and prod upgrade in the same action — a genuinely bad property for a two-person team with no staging.
- **Secret separation, which is the decisive argument.** In one cluster, prod's `ERSERVER_OPERATOR_TOKEN` — a cross-tenant operator credential — and `ERWEB_CREDENTIAL_KEY`, which unseals every org's stored API keys, would sit in the same etcd as namespaces where developers hold admin. Namespace RBAC separates them, but any cluster-scoped escape crosses into prod, and dev holds real data so the incentive exists.

That last point is also a consistency argument. §8.2 justifies structural separation on the grounds that it beats policy correctness. A single cluster adopts precisely the posture that reasoning rejects; if it were acceptable at the Kubernetes layer, a single AWS account should have been acceptable too. (D-ACCT then accepted exactly the weaker account posture — so the two-cluster boundary is now the one structural separation the design still keeps, which is part of why it is kept.)

**Rejected alternative.** A single cluster for all environments with namespace separation.

**Date.** 2026-10-03.

---

## D-AUTOMODE — EKS Auto Mode, chosen (D1) — resolves old §18.3

**Decision.** EKS Auto Mode. (Design decision D1; the old open item §18.3 is closed.)

**Context.** Auto Mode manages Karpenter and the core addons, removing both the system node group and the operational burden of upgrading them. For a two-person team that trade — roughly a 12% EC2 surcharge against not operating Karpenter — is plausibly worth taking. The second revision declined it on one question: whether §5.3's `instanceStorePolicy: RAID0` is expressible. **The answer is yes — and better than expressible, automatic.** Auto Mode formats instance-store NVMe for ephemeral use on its own, striping RAID0 across multiple drives, with no field to set; constrain the NodePool to NVMe families and `emptyDir` lands on it. Auto Mode also enforces IMDSv2 with hop limit 1 unconditionally (§5.6) and ships SOCI parallel pull on by default (§11.1) — two things this design wants anyway.

The resolving check (D1, 2026-10-10): Auto Mode honours `karpenter.sh/do-not-disrupt` (the node is preserved while the annotated pod runs, across every consolidation policy), and its NodePool surface exposes `disruption.consolidationPolicy: WhenEmpty` with `consolidateAfter` — the §5.1 runner-path controls survive verbatim. The surcharge arithmetic at this footprint: the fee is ~12% of the **on-demand** rate per instance-hour and is charged even on Spot capacity — a poor percentage on discounted runners, but in absolute terms roughly $9/mo for the always-on app node and cents per runner-hour, against not operating Karpenter's upgrade treadmill solo.

**What it still costs.** The management fee; no custom AMIs or `blockDeviceMappings`, so §11.1's snapshot-prebake rung is off the table; and since April 2026 Auto Mode's managed instances are hidden from `DescribeInstances`, which confuses inventory tooling.

**Rejected alternative.** Self-managed Karpenter (the second revision's choice), which the design falls back to if the Auto Mode fee ever justifies it — which also reopens §11.1's prebake rung.

**Revisit trigger.** If the Auto Mode fee line in the CUR exceeds ~$50/mo, re-run this decision against self-managed Karpenter.

**Date.** Declined rev 2 (2026-10-03); reopened rev 3 (2026-10-03); chosen 2026-10-10.

---

## D-EDGE — Dev behind an internal ALB over Tailscale; not internet-facing, not managed VPN, not ALB OIDC

**Decision.** Dev: an internal ALB, reached over Tailscale (a subnet router on a `t4g.nano` advertising the VPC CIDR, resolving the private zone via the VPC resolver). Prod, when it exists: internet-facing behind WAF with rate-based rules. `er-api` is on no load balancer in any environment.

**Context.** The earlier revisions specified Ingresses and an ALB and never said *from where* — which, unexamined, meant internet-facing. For dev that is a finding, not an oversight to forgive: §13 deliberately fills developer namespaces with real production records, and an internet-facing ALB leaves the Next.js session check as the only thing between those records and the internet. §8.2 spends an entire account boundary refusing exactly this posture — one policy away from exposure — so the edge deserves the same reasoning. The Tailscale subnet router is roughly $4/mo, and Tailscale's free tier covers three users. For prod the boundary moves from network to WAF plus application auth: the frontend exposes five unauthenticated paths — six once SSO lands, with [frontend-design.md](frontend-design.md) §2.4 adding `/api/auth/sso` — and three of them (`/api/auth/login`, `/api/auth/password-reset`, `/api/invites`) accept unauthenticated writes with no in-app rate limiting; WAF rate rules scope to URI paths with thresholds down to 10 requests per window, at roughly $8/mo all-in.

**Rejected alternatives.**
- **Managed VPN.** Client VPN has a ~$73/mo idle floor before connection hours; Verified Access runs ~$197/mo per application. Both price themselves out at this scale against the ~$4 tailnet.
- **ALB OIDC authentication against Identity Center.** It does not work here: Identity Center cannot act as ALB's OIDC provider — it exposes no userinfo endpoint — so that path requires a Cognito user pool federated to Identity Center over SAML, which is real plumbing to approximate what the tailnet gives structurally.
- **Internet-facing dev** (the earlier revisions' unstated default), refused for the real-records reason above.

**Date.** 2026-10-03.

---

## D-TENANCY — Containment by process/catalog/lock, not namespace-per-tenant

**Decision.** In prod every tenant's runs execute as Jobs in one platform namespace; a run's isolation comes from process-scoped env, ephemerality, a dedicated catalog database per tenant, and the session-scoped advisory writer lock — not a namespace boundary. The per-namespace split in §12 is per *developer*, not per tenant.

**Context.** The Kubernetes-shaped intuition does not match the tenancy model: [backend-design.md](backend-design.md) §7 makes a tenant a database, a schema, and an S3 prefix — not a namespace.

**Rejected alternative.** Namespace-per-tenant would add hard RBAC, NetworkPolicy, and quota boundaries, at the cost of namespace proliferation across hundreds of tenants, per-namespace secret plumbing, and a dispatcher needing cluster-wide job-create rights — which trades away more isolation than it buys (a dispatcher able to create pods cluster-wide undercuts D-CLUSTER's separate-clusters argument). It is the right shape for the enterprise tier that [backend-design.md](backend-design.md) §7 already earmarks for dedicated buckets and STS-scoped credentials, and not before. The forward-looking seam: once §9.2's IRSA work lands, the per-run pod can carry a per-tenant ServiceAccount scoped to that tenant's prefix alone — the "per-tenant STS credentials" item in [backend-design.md](backend-design.md) §15 phase 4 — so the Job spec anticipates a ServiceAccount parameter even while every run shares one.

**Date.** 2026-10-03.

---

## D-SIZING — vCPU-tiered resource classes, not memory-tiered

**Decision.** Resource classes tiered on vCPU with a near-constant memory floor, sized per run from the record count, carried on the **job row** (`jobs.resource_class`) rather than merged into `orgs.env`. Authority: [server/src/erserver/sizing.py](../server/src/erserver/sizing.py). [backend-design.md](backend-design.md) §7 has been corrected to match (§18.9 resolved — the spec and the implementation now agree).

**Context.** The measurements say the thing to scale is CPU, not memory, and that inverts the resource classes the design started from:

| Measurement | Source | What it shows |
|---|---|---|
| 10M-record full reload: 2 vCPU, 2 DuckDB threads, **4 GB** DuckDB limit, 10 GiB container. Peak container memory **9.37 GiB** | [performance.md](performance.md) | Ten million records completed in a 10 GiB container |
| Same corpus, 2 → 6 threads at the **same 4 GB** limit: 6,458s → 3,857s (**−40%**), peak memory 9.14 GiB. Work in progress, one pass per arm | 2-thread arm: [measurements/splink5-20260922.json](measurements/splink5-20260922.json); 6-thread arm: [performance.md](performance.md) | Threads buy time. Memory stayed flat while runtime fell 40% |
| 1M, 2 threads/4 GB → 6 threads/6 GB: 96.2s → 75.1s (−22%), peak 4.06 → 4.26 GiB. Work in progress; the DuckDB limit changed with the threads | [performance.md](performance.md) | Same shape an order of magnitude down |

The reason memory stays flat is that DuckDB **spills**, and spill is why §5.3 exists. A larger corpus does not need proportionally more RAM; it needs somewhere to put 141 GiB of temporary files. Local NVMe comes free with the instance while RAM is billed, so the economically correct posture is to *embrace* spilling to instance store rather than buying memory to avoid it. [backend-design.md](backend-design.md) §7's original classes — S 8 GiB, M 24 GiB, **L 64 GiB** — were over-provisioned by roughly 6× on the dimension that does not matter (a 64 GiB pod would spend most of its allocation idle while still taking 1h04m) and silent on the dimension that does.

**Why the job row, not `orgs.env`.** Dynamic sizing makes the §2 API-pod inheritance problem worse: if a tenant's env is set to 16 threads for its nightly 10M run, the API pods serving that tenant's read path inherit 16 threads across up to `_POOL_CAPACITY = 8` warm attaches. Per-run sizing therefore rides the job row until the [backend-design.md](backend-design.md) §8 split between tenant and API-pod settings is implemented (§18.10).

**Rejected alternative.** Memory-tiered resource classes (S 8 / M 24 / L 64 GiB), following the original [backend-design.md](backend-design.md) §7. The record-count-to-class mapping in [benchmarks/scales.yaml](../benchmarks/scales.yaml) transfers; its memory figures do not, because its envelopes are deliberately generous so a benchmark never OOMs mid-run (the `1m` scale declares 12 CPU / 56 GiB) whereas production sizing should track measured *peaks*.

**Two honest limits.** The thread-scaling measurement stops at 6 threads, so the knee above 6 is unknown — and unlikely to be linear, because the same run showed **reconcile getting 14% *slower*** at 6 threads, being a Python graph step that does not parallelize with DuckDB threads (§18.2). And the `le=32` bound on `duckdb_threads` caps the useful instance at 32 vCPU, which is what makes `c6id.8xlarge` the top of the ladder.

**Date.** 2026-10-03; backend-design.md §7 corrected subsequently (§18.9).

---

## D-DISRUPT — Karpenter disruption settings, not the scale-to-zero defaults

**Decision.** `app` pool `WhenEmptyOrUnderutilized` / `1m`; runner pools **`WhenEmpty`** / `30s` with `karpenter.sh/do-not-disrupt: "true"` on the runner pods; `expireAfter: 72h` for AMI hygiene.

**Context.** The first draft said "all NodePools scale to zero" and specified no disruption settings at all. The defaults are actively wrong for this workload, and the wrong choice here is invisible — it shows up as jobs taking longer, not as errors. The runner pools must not use `WhenEmptyOrUnderutilized`: that is the setting reached for to save money, and it permits Karpenter to drain a busy node to bin-pack onto a cheaper one. On a 10M-row run that discards up to an hour of work. The retry policy in §2 makes it *survivable* — exit code `None`, requeue with `--resume` — which is precisely why it would register as unexplained slowness rather than as an incident. `karpenter.sh/do-not-disrupt` on the runner pods blocks voluntary disruption for the life of the job; nodes still empty ~30s after the pod exits; `expireAfter` handles AMI hygiene, since a do-not-disrupt pod otherwise pins a node indefinitely. Spot interruption is *involuntary* and cannot be annotated away — Karpenter drains on the two-minute notice, `--resume` picks up at the last completed stage, and that is the trade accepted in exchange for spot pricing.

**Rejected alternative.** The Karpenter defaults / `WhenEmptyOrUnderutilized` on the runner pools.

**Date.** 2026-10-03.

---

## D-SPILL — Explicit ephemeral-storage sizing (the eviction trap)

**Decision.** Runner pods carry an explicit `requests.ephemeral-storage` (per class) and an `emptyDir.sizeLimit` generous enough for the real spill plus headroom but below the node's capacity — roughly 400 GiB per L runner on a 950 GB `c6id.4xlarge`, which fits two.

**Context.** Both runner NodePools set `instanceStorePolicy: RAID0`, striping the local NVMe and making it the kubelet's ephemeral-storage root, so an `emptyDir` lands on NVMe automatically. **The first draft stopped there, and that was incomplete.** An `emptyDir` counts against the pod's ephemeral-storage accounting, so the sizing has to be explicit in two places or the design has a trap in it:

- **With no limit**, one runaway run fills the node's NVMe and the kubelet evicts *every* pod on it under disk pressure, including other tenants' runners.
- **With a limit set too low**, the runner itself is evicted partway through. Eviction yields exit code `None`, which requeues with `--resume` — and then evicts again at the same point. `max_attempts` (default 3) bounds this, so it is not an infinite loop, but it is three discarded runs and a job that fails for a reason the logs attribute to the OS rather than to a misconfigured volume.

The explicit request is also what lets the scheduler avoid stacking two spill-heavy pods onto one node. Crucially, the 141 GiB spill figure was measured at a **4 GB** DuckDB memory limit, and §6.5 keeps the L class at 8 GB rather than the 64 GiB the original resource classes called for — so the measurement was taken near the configuration this design actually runs, and transfers. Had the design kept a 64 GiB DuckDB limit, this number would have been an extrapolation across a 16× change in buffer size and worth nothing. Spilling heavily to free instance-store NVMe instead of buying RAM to avoid it is the deliberate trade (D-SIZING).

**Rejected alternative.** An `emptyDir` on NVMe with no explicit ephemeral-storage request/limit (the first draft).

**Date.** 2026-10-03.

---

## D-PROGRESS — The runner writes its own progress, not dispatcher log-streaming

**Decision.** The runner writes its own progress to the control-plane database, rather than the dispatcher streaming pod logs.

**Context.** Today `launch_runner` reads the subprocess's **stderr pipe** directly, parsing one JSON record per stage and writing it into `jobs.progress` while the run is live. A Job pod has no pipe to the dispatcher, so this is the one place where substituting a Job is not transparent. Two options were weighed:

1. **Dispatcher streams pod logs** via `read_namespaced_pod_log(follow=True)`. Preserves the existing parsing code, but holds one long-lived HTTP stream per concurrent run, loses progress if the dispatcher restarts mid-run, and is vulnerable to log rotation on a multi-hour job.
2. **The runner writes its own progress** to the control-plane database. The runner already holds a DSN and already emits exactly these records; this makes the write direct instead of relayed.

Option 2 wins: it survives dispatcher restarts, removes N long-lived streams, and makes the dispatcher a pure scheduler rather than also a log relay. It is a small change to [server/src/erserver/runner.py](../server/src/erserver/runner.py) and it is on the same seam that §10 needs for trace context — so the two were designed together (§18.5 recorded the decision point).

**Rejected alternative.** Dispatcher streams pod logs (option 1).

**Date.** 2026-10-03.

---

## D-JOBSPEC — `backoffLimit: 0` and the mandatory EFS mount

**Decision.** The Job carries `backoffLimit: 0` / `restartPolicy: Never`, and mounts the EFS access point at `ERSERVER_CONFIG_ROOT` / `ERSERVER_DROP_ROOT`.

**Context.** `backoffLimit: 0` is the single most important field. The default of 6 would have Kubernetes retry a failed run on its own schedule, with no knowledge of the exit-code taxonomy — re-running a config error six times, and worse, re-running a partially-completed pipeline *without* `--resume`. **The EFS mount was omitted by the second revision**, and its absence fails late: the runner's first act is opening the config file by path ([server/src/erserver/runner.py:31](../server/src/erserver/runner.py)), and an incremental import lists its CSV drops from `drop_root` at ingest time ([src/er/ingest/sources.py:146](../src/er/ingest/sources.py)). Half an escape hatch exists and is worth recording: config *content* could travel inline — `configsvc.validate_yaml` already round-trips inline YAML through a temp file — but drops cannot leave the filesystem, because `storage.drop_dir` must be an absolute local path ([src/er/config/schema.py:285](../src/er/config/schema.py)) and discovery is a literal directory listing. Inline config is a later optimization, not a way out of the mount. The dispatcher needs the same mount for its own reasons (§2): `drain_staged` re-loads an org's config on every idle pass.

**Rejected alternative.** The Kubernetes default `backoffLimit: 6`; and the second revision's Job spec with no EFS mount.

**Date.** 2026-10-03.

---

## D-VERSIONING — Lake bucket lifecycle policy (omitted by the first draft)

**Decision.** S3 versioning on the lake bucket carries a lifecycle policy: noncurrent versions expire after 30 days, incomplete multipart uploads after 7.

**Context.** The first draft enabled versioning and omitted the lifecycle. Every pipeline run rewrites parquet, so versioning without noncurrent-version expiration grows without bound; a run killed mid-write otherwise leaves multipart parts that bill silently.

**Date.** 2026-10-03.

---

## D-ROTATION — Secret rotation needs a restart (a reloader), and two keys must not auto-rotate

**Decision.** A reloader annotation triggers a rolling restart on a projected-Secret change; dispatcher rotation is *scheduled* rather than arbitrary because it is `replicas: 1` / `strategy: Recreate`; `ERWEB_CREDENTIAL_KEY` is manual-with-migration and deliberately not scheduled.

**Context.** The first draft said Aurora passwords "use Secrets Manager managed rotation" and left it there. That combination breaks the platform: a process environment is fixed at exec time, so on rotation ESO updates the Kubernetes Secret while the running pods keep the stale DSN and connections begin failing. `ERWEB_CREDENTIAL_KEY` unseals every org's stored erserver API key in `erweb.org_credentials` (AES-256-GCM with AAD `"<org>\0<role>"`, per [frontend/src/lib/db/crypto.ts](../frontend/src/lib/db/crypto.ts)); rotating it without a re-encrypt pass bricks the vault.

**Rejected alternative.** Secrets Manager managed rotation with no reloader (the first draft).

**Date.** 2026-10-03.

---

## D-SES — SES SMTP is a second static credential with no rotation story that fixes it

**Decision.** A second IAM user exists for SES SMTP (`ses:SendRawEmail` only, SMTP password derived from its access key, in Secrets Manager, rotated under the reloader rules) until the mailer is ported to the SES API in Phase 4.

**Context.** Email was treated as a future feature by the earlier revisions; it is not. [server/src/erserver/mailer.py](../server/src/erserver/mailer.py) is a complete sender behind a durable `email_outbox` table that the dispatcher's leader tick drains with retry and backoff, and live flows depend on it (invites, password reset, `job_failed` and `report_completed` notifications). It is dormant today only because nothing sets `ERSERVER_SMTP_URL` — which in a cloud environment means every invite parks silently in the outbox. SES's SMTP interface **does not accept credentials derived from temporary credentials**; AWS documents this flatly, so IRSA can never back the SMTP path, no matter how the §9.2 cluster-identity work progresses. This is the amendment to §8.1's "one long-lived key" claim: two static keys, both with named Phase-4 exits.

**Rejected alternative.** An SMTP interface endpoint (not worth it at this scale — it has had availability-zone gaps in some regions, and port 25 is throttled from EC2 regardless; pods reach the SMTP endpoint through the NAT on 587). And IRSA for the SMTP path — structurally impossible.

**Date.** 2026-10-03.

---

## D-KMS — Customer-managed keys from day one; ECR and the cluster-API key are exceptions

**Decision.** Every S3 bucket and every Aurora cluster gets a customer-managed key (CMK) at creation. ECR is AES256. The EKS cluster-API encryption key is left as the AWS-owned default — a CMK is deliberately *not* associated.

**Context.** Two specific reasons for CMKs on S3/Aurora: an RDS snapshot encrypted with the default `aws/rds` key **cannot be shared across accounts** at all; and a dev principal cannot read an SSE-KMS prod object unless the prod key policy grants it. Retrofitting a CMK onto a live prod database means a full snapshot-copy-and-restore migration with downtime; doing it at creation costs nothing.

**Rejected alternatives.**
- **CMK on ECR** — rejected (AES256 instead), because images are built from public source, hold no records, and a CMK would require a key plus cross-account grants for no confidentiality gain.
- **CMK on the EKS cluster API data** — rejected. EKS has envelope-encrypted all Kubernetes API data with an AWS-owned key on every 1.28+ cluster since March 2025. A customer-managed key is deliberately not associated: association is irreversible, and a disabled or deleted CMK degrades the cluster unrecoverably — risk with no cross-account payoff. Same reasoning as the ECR exception.

**Date.** 2026-10-03.

---

## D-RETENTION — Log retention is split: compliance window vs cost

**Decision.** Audit-relevant trails (CloudTrail management + data events, `erweb.audit`, erserver `audit_log`) retain **≥ 400 days**; application debug logs retain 30 days.

**Context.** The first draft applied 30 days of retention uniformly. For a six-month SOC 2 Type 2 that leaves five months with no evidence — and you cannot retroactively produce six months of logs. Retention is a **compliance parameter for audit trails and a cost parameter for everything else**, and conflating them is what makes people either overpay or fail the window. 400 days is a year plus margin, so a window never runs off the end.

**Rejected alternative.** A uniform 30-day retention (the first draft).

**Date.** 2026-10-03.

---

## D-OTEL — OpenTelemetry to one collector; one storage backend rejected; LGTM disqualified

**Decision.** OpenTelemetry to a single collector (a Deployment gateway plus a DaemonSet for log collection), exporter swappable; start with CloudWatch + X-Ray via ADOT. One *collection* layer, but **not** one storage backend for all three signals.

**Context.** OTel is the right foundation for four reasons specific to this system:

1. **Three runtimes, one standard.** The engine is Python, the control plane is Python/FastAPI, the BFF is Node/Next.js. OTel is the only instrumentation standard that covers all three over one wire protocol into one collector.
2. **The interesting unit of work crosses five process boundaries.** A user clicking "run" traverses Next.js → BFF → erserver API → a `jobs` row → the dispatcher → a Job pod → a dbt subprocess → DuckDB. Distributed tracing is the one thing that is genuinely hard to retrofit, and §6 makes it harder by putting the runner in a different pod.
3. **Vendor neutrality is a product requirement, not a preference.** [backend-design.md](backend-design.md) §15 phase 5 contemplates a self-hosted option. A collector lets a self-hoster point at their own backend; CloudWatch-native instrumentation does not.
4. **The engine already emits the right shape.** Stage records carry `tenant/job/run/stage` and typed counters; they become span attributes almost directly.

Two decisions get conflated and have different answers. **One collection layer: yes** — it is the one embedded in application code, so changing it later means touching every service. **One storage backend for all three signals: no** — metrics are small/regular/aggregated and want long retention; logs are large/bursty and want full-text search with short retention; traces want ID lookup over sampled data. The known next step is splitting by signal (logs stay in CloudWatch, in-account; metrics and traces to Grafana Cloud or AMP+Grafana), following the PII boundary rather than convenience. Start with CloudWatch + X-Ray via ADOT not because it is the best backend but because the whole argument for OTel is that the exporter is swappable and the cost at one developer is a few dollars — choosing the boring option first is the cheapest way to discover what you actually need to query.

**Rejected alternatives.**
- **Self-hosted LGTM in-cluster** — disqualified. Four stateful services to operate and back up for a two-person team, running on the same cluster and competing for the nodes §5.3 sized for 141 GiB of spill. An observability stack that degrades when the system it observes gets busy is worse than none.
- **Deriving metrics from logs** (CloudWatch metric filters / Logs Insights aggregations) — a trap: you pay log-ingest rates for every data point. Metrics are emitted as metrics.
- **`tenant` as a metric-series label** — the classic multi-tenant cost blowup; use `resource_class` as the metric dimension and get per-tenant detail from traces and the `runs` table.
- **SaaS backends for logs** — conflicts with §8.5, because logs are the signal that could carry record values.

**Date.** 2026-10-03.

---

## D-PULL — The cold-node pull ladder; "let the node cache do the work" was defeated

**Decision, revised 2026-10-24: the ladder is closed at rung zero.** The deciding measurement this decision always deferred to now exists — [`measurements/runner-startup-20261024.json`](measurements/runner-startup-20261024.json), taken with `scripts/measure_runner_startup.sh` on `er-dev` — and it says the pull is **~2 seconds** of an approximately 30-second cold start. No rung is worth climbing for that:

- **Rung 1, SOCI parallel pull, was never available.** The enabling Auto Mode release note (2025-11-19) scopes it to G, P and Trn instances with local NVMe. The runner pools are `c6id`. "On by default" was wrong, and there is no setting that turns it on (§11.1).
- **Rung 2, the runtime/CI image split, is solving the wrong term.** It would shave part of two seconds. The image did shrink — it now carries one binary instead of three, each of which had been statically linking its own DuckDB — but that was taken for the sake of shipping only what runs, not for the pull.
- **Rung 3, snapshot prebake, costs Auto Mode** (D-AUTOMODE) to chase the same two seconds. Closed.

**What the measurement found instead.** The larger of the two terms under our control was the **DuckDB extension fetch — roughly 5 seconds, about three times the pull.** `duckdb` is pinned with the plain `bundled` feature, without `parquet` or `json`, and the bundled build enables autoinstall, so only `core_functions` is statically linked and a cold process downloaded `ducklake`, `parquet`, `json` and `icu` from `extensions.duckdb.org` before reading a row. That is now baked into the image and measured at effectively zero, and it also removes a hard runtime dependency on an external repository and takes the traffic off the NAT gateway. This is the opposite of what the ladder assumed: the bytes the design worried about were the image's, and the ones that mattered were the extensions'.

**Also stale:** the ~90 s node provision in plan-to-aws 1.3. NodeClaim to Ready is now ~14 s.

**Superseded decision (2026-10-03).** Climb a mitigation ladder, cheapest first, only as far as measurement justifies: (1) SOCI parallel pull — effectively free, on by default under Auto Mode; (2) a runtime/CI image split; (3) snapshot-prebaked images (standard Karpenter only — unavailable under Auto Mode).

**Context.** The second revision said to pin the tag "and let the node cache do the work." For the runner pools there is no node cache: §5.1 empties a runner node ~30 seconds after its pod exits, so nearly every run starts on a freshly provisioned node pulling `er-pipeline` from scratch. The bytes are already cheap — layer blobs come from S3 over the gateway endpoint (D-ENDPOINTS) — but the minutes are real on an image that carries a full venv (splink, pyarrow, duckdb, dbt), baked DuckDB extensions, compiled MinIO binaries, and — because the compose `pipeline` service's command *is* pytest — the tests, fixtures, and dev dependency group too. The runtime/CI split is a contract change, not just a Dockerfile change: `test_dockerfile_contract.py` exists to keep the compose image testable, so the split means two targets and an updated contract test. Snapshot prebake adds an artifact to keep fresh, so it is the last rung. The deciding measurement — cold-node pull time for the current image — does not exist yet (§18.16).

**Rejected alternative.** "Pin the tag and let the node cache do the work" (the second revision) — there is no cache on scale-to-zero runner nodes.

**Date.** 2026-10-03.

---

## D-REGISTRY — Promotion by digest; registry replication removed with the account collapse

**Decision.** ECR repositories live in the account's single registry (tag immutability, scan-on-push, last-30-images lifecycle). Promotion is **by digest, never by tag**.

**Context.** The prior nonprod-source-of-truth → prod-replica arrangement collapsed with the accounts (D-ACCT); replication and the destination registry policy are no longer needed. The byte-identity guarantee survives without replication because it never depended on it: a prod deploy pins the exact digest CI built and tested, and an immutable tag plus a digest proves what ran in prod came from a specific reviewed commit — better CC8 evidence than most organisations can produce.

**Rejected alternative.** Cross-account registry replication (nonprod source → prod replica), obsolete under a single account.

**Date.** 2026-10-10.

---

## D-DEPLOY — Who applies Terraform and Helm (the guardrail's last clause)

**Decision.** Terraform plans on every PR under a read-only role and applies on `main` under an apply role, both assumed through §11's GitHub OIDC provider with a `main`-only `sub` restriction; the prod-scoped apply role's trust policy names exactly two principals — the CI apply role and `ProdAdmin` as break-glass. Helm: dev platform releases apply from CI on `main`, `make dev-up` stays a human command, prod deploys are manually triggered and pass an image **digest**.

**Context.** The earlier revisions defined what runs and never who applies it — and the gap was load-bearing, because two §8 claims quietly depend on the answer: promote-by-digest is only CC8 *evidence* if the deploy path enforces it, and the §8.2 exemption for Terraform is only safe if assuming the Terraform role is harder than the guard it bypasses. A PR from a fork runs attacker-authored workflow code, which is why PRs are excluded from the push/apply credential. That trust policy is the §8.2 guardrail's last clause.

**Rejected alternative.** Argo CD / GitOps for the job path — out of scope (see D-ARGO); this is the minimum that turns promote-by-digest from a convention into a property.

**Date.** 2026-10-03.

---

## D-COSTTOOL — Kubecost / OpenCost not adopted initially

**Decision.** Use EKS Split Cost Allocation Data (per-pod CUR rows) plus a per-run cost figure computed in Postgres; do not stand up Kubecost/OpenCost yet.

**Context.** OpenCost is the right shape — per-namespace and per-label pod cost — but it wants Prometheus, which §10.5 deliberately does not run, so adopting it means standing up a metrics store for one consumer. Split cost allocation now covers the same ground natively: per-developer cost falls out of namespace-per-developer (§12), and per-tenant compute cost is tractable because §6 makes a run a dedicated single-tenant pod. The internal per-run figure is allocated at a blended rate (within ~10%, not billed-to-the-penny), queryable next to the business counters; the CUR stays the authority for the actual bill.

**Rejected alternative.** Kubecost/OpenCost on day one — revisit if real-time cost visibility becomes worth a metrics store, or if per-label (rather than per-namespace) CUR granularity turns out not to be available (§18.11).

**Date.** 2026-10-03.

---

## Resolved open items (moved out of §18)

**§18.3 — Does EKS Auto Mode support the §5.1/§5.3 node controls?** Resolved 2026-10-10 — yes on all counts, and D1 is decided: Auto Mode. RAID0 NVMe is automatic; `do-not-disrupt` is honoured across consolidation policies; NodePools expose `consolidationPolicy: WhenEmpty` + `consolidateAfter`. Fee ~12% of on-demand per instance-hour (charged even on Spot) — ~$10–20/mo at this footprint. Full decision and revisit trigger: **D-AUTOMODE** / §5.5.

**§18.8 — Hosted zone and domain.** Resolved 2026-10-10: **`dupezero.com`, registered at Cloudflare.** Cloudflare Registrar pins the apex to Cloudflare's nameservers, so **Cloudflare is the DNS plane** — no Route 53 zone. Consequences, all mechanical: external-dns runs its Cloudflare provider (scoped API token from Secrets Manager, `--domain-filter=dev.dupezero.com`, `--txt-owner-id` per cluster) and must create **DNS-only (unproxied) records**, since dev hostnames resolve to an internal, tailnet-reached ALB that Cloudflare's proxy could never reach; ACM stays the certificate authority (the wildcard dev cert validates via a CNAME the Terraform `cloudflare` provider writes); SES Easy-DKIM CNAMEs and MAIL FROM records land in the Cloudflare zone the same way.

**§18.9 — Resource classes revised in [backend-design.md](backend-design.md) §7, not just implemented.** Resolved: `backend-design.md` §7 now specifies the vCPU- and spill-tiered classes (authority `server/src/erserver/sizing.py`), so the two documents no longer disagree. Rationale: **D-SIZING**.
