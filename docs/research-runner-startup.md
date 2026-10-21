# Research: making runner pods start faster without paying for warm capacity

*2026-10-21. Research only; nothing here is implemented. Companion to
[infrastructure.md](infrastructure.md) §5/§6/§11 and
[infrastructure-decisions.md](infrastructure-decisions.md) D-PULL / D-DISRUPT / D-AUTOMODE.*

## 1. What the code and infra actually do today

**The slow part is not the pod. It is the node.** The design (§6) runs each
pipeline run as one Kubernetes Job on a tainted runner NodePool
(`infra/k8s/nodepools/runner-sm.yaml`, `runner-l.yaml`). Those pools are
`consolidationPolicy: WhenEmpty` / `consolidateAfter: 30s`, so a runner node is
reclaimed ~30 s after its pod exits. Consequently **nearly every run provisions a
fresh EC2 instance**: Karpenter launches it, Bottlerocket boots, the kubelet
joins, Auto Mode stripes the NVMe, the image is pulled cold, and only then does
the pod start. D-PULL already records this ("there is no node cache on
scale-to-zero runner nodes"). Measured in this repo:

| Observation | Where |
|---|---|
| A probe pod provisioned a node in ~90 s | plan-to-aws.md 1.3 |
| A `provision` job ran 77 s pod-to-complete on a fresh NVMe node | plan-to-aws.md 2.3 |
| A 1M-record full reload takes ~92 s; a 100k incremental onto 10M took 2m50s | performance.md |

So for S-class work the node spin-up is on the order of the run itself, and
for a chain of jobs (the Phase 2 evidence was `provision → 3 imports → train →
run_all_full`, each its own Job) every link pays the full cold start, because
the previous node was already reclaimed 30 s after its pod exited.

**Why separate compute at all** (backend-design.md §4, infrastructure.md §6):
tenant isolation is process-scoped env (`ER_*`), the Postgres advisory writer
lock is session-scoped and released by the pod dying, and sizing is per run on
vCPU + local NVMe spill (S 2 vCPU / 40 GiB, M 6 / 150 GiB, L 12–16 / 400 GiB).
Memory is nearly flat (~6–14 GiB) because DuckDB spills; the thing that scales
is CPU and scratch disk, and that only bin-packs if the allocation is per run.
Those reasons are sound and nothing below argues against pod-per-run for M and
L.

**A discrepancy worth knowing.** The Rust port runs jobs *in-process*: the
`web-api` Deployment sets `ER_DISPATCH_IN_PROCESS=1` and
`er-server/src/dispatcher.rs` executes the `er-cli` op on a blocking thread
inside the API pod ("no per-run k8s Jobs"). `values.yaml` still carries
`dispatcher.launcher: kubernetes` and an `images.runner` block, but nothing in
`rust/` reads them, and `er-server/src/lib.rs` lists the k8s launcher as
"infrastructure-bound / not yet ported". So the pod-per-run path described in
the question is the Python-era launcher (plan-to-aws 2.3) and the target design,
not what the current Rust chart does. The recommendation below is written for
the target design, and it also tells you what to keep from the in-process path.

**Two startup taxes that are in the code, not the cluster:**

1. `Session::attach_ducklake` runs `INSTALL ducklake; LOAD ducklake;` on every
   lake open (`dedupe-core/src/session/mod.rs:266`). On a fresh node with an
   empty `~/.duckdb`, that is a network fetch from the DuckDB extension repo per
   run. `rust/Dockerfile` notes baking the extension is a follow-up. The
   `extension_directory` option already exists; `attach_ducklake` just does not
   use it.
2. `rust/Dockerfile` ships three binaries (`er-server`, `er-dispatcher`, `er`),
   each statically linking its own copy of DuckDB (`bundled`). The image pays for
   DuckDB three times; a single multicall binary (or shipping only `er` in a
   runner image) would cut the pull by roughly two-thirds. Measure with
   `docker image inspect` before deciding; §18.16 still has no number.

## 2. The options, with what they cost

Pricing is us-east-2, approximate (third-party aggregator, Oct 2026):
`c6id.2xlarge` ≈ $0.40/h on-demand, ≈ $0.08/h spot; the Auto Mode fee is ~12%
of the **on-demand** rate (≈ $0.05/h) and is charged even on spot. A warm spot
runner node therefore costs ≈ $0.13/h ≈ **$95/mo**; on-demand ≈ $0.45/h ≈
**$330/mo**.

| # | Option | Cold-start saved | Monthly cost | Verdict |
|---|---|---|---|---|
| A | Run S-class jobs in-process (dispatcher already does this) | all of it (0 s) | $0 | **Do this** |
| B | Bake the DuckLake extension; single-binary runner image | seconds per run, every run | $0 | **Do this** |
| C | Raise runner `consolidateAfter` from 30 s to ~5–10 min | all of it for the 2nd..nth job in a chain | ≈ $0.01–0.02 per run on spot | **Do this** |
| D | Pause-pod overprovisioning (one placeholder on `runner-sm`) | all of it, for one job at a time | ≈ $95/mo spot, $330 on-demand | Only if C is not enough |
| E | Static-capacity NodePool (`spec.replicas: 1`, Auto Mode since 2025-11-19) | same as D | same as D | Same as D, simpler, but one-way (cannot revert to dynamic) |
| F | EKS Fargate for S class | maybe 20–40 s | per-second billing, no idle | No: no NVMe, no spot, image pulled every time |
| G | KEDA ScaledJob | 0 s | free, but more moving parts | No: it triggers Jobs; it does not pre-warm anything |
| H | Nix instead of Docker | 0 s | engineering time | No: the image is already a static binary on `bookworm-slim`; Nix changes nothing about node boot |
| I | Self-managed Karpenter + snapshot-prebaked AMI | ~pull time (~10–20 s) | loses Auto Mode, adds an AMI to keep fresh | No at this size; the pull is not the dominant cost |
| J | Bottlerocket / different AMI | nothing | n/a | Auto Mode nodes already run Bottlerocket |

### A. Tiered dispatch: in-process for S, Job for M and L

The in-process path exists, is tested, and has zero startup. The concern that
led to pod-per-run was CPU isolation and per-run sizing. For S class (≤100k
records, 2 vCPU, 4 GB DuckDB limit, 40 GiB spill) that concern is small enough
to buy back with a few lines:

- Decide at claim time from `jobs.resource_class` (already on the row). S → run
  in-process; M/L → launch a Job.
- Give the in-process path a bounded thread count and memory limit
  (`ER_DUCKDB_THREADS=2`, `ER_DUCKDB_MEMORY_LIMIT=2GB`), a `sizeLimit` on the
  `duckdb-tmp` emptyDir large enough for S spill (the chart has 2 Gi today;
  S class is sized at 40 GiB — the app node has no NVMe, so either cap S
  tighter for the in-process path or mount a gp3 scratch volume), and a
  concurrency of one so two tenants' runs cannot both land on the API pod.
- Move the dispatcher back to its own Deployment (the chart did this before the
  merge) so a run cannot starve the HTTP server on CPU; it still shares the
  always-on `app` node, which is what makes it free.

The isolation argument survives: it is one tenant per *process-scoped
environment*, which `run_job` already honours by building the session from the
org row, and the advisory lock is still released when the session drops.

### B. Remove the per-run network fetch and shrink the image

- Bake `ducklake` into the image under `extension_directory` and make
  `attach_ducklake` do `LOAD` only (the `install_extensions` flag already
  exists for the community extensions; extend it). Saves one HTTPS round-trip
  plus a download on every run and removes a dependency on the extension repo
  being reachable from the runner (the Dockerfile comment calls this out).
- Build a runner-only image (just `er`), or a multicall binary. Measure the
  pull first: SOCI parallel pull on Auto Mode is **only** enabled for G, P and
  Trn families (release note 2025-11-19), so the infrastructure.md §11.1 claim
  that it is "on by default" does not apply to `c6id`. Auto Mode did ship
  "improved image pull speed for instance types with local instance storage"
  (2025-04-11), which does apply.

### C. Let a runner node linger a few minutes

This is the cheapest form of "warm" that exists: a node that has just finished
one job stays up for the next one. The whole chain
`provision → imports → train → run_all` then pays one cold start instead of
six. Cost is the linger window times the instance rate when no second job
arrives:

| `consolidateAfter` | Idle cost per run (spot, incl. fee ≈ $0.13/h) | On-demand |
|---|---|---|
| 30 s (today) | $0.001 | $0.004 |
| 5 min | $0.011 | $0.038 |
| 10 min | $0.022 | $0.075 |

The AWS cost-control page for bursty workloads says to keep `consolidateAfter`
short, which is correct advice for a fleet of CI runners; at one or two nodes a
10-minute linger is a few dollars a month and removes the worst user-visible
case (back-to-back jobs). Keep `WhenEmpty` and `do-not-disrupt` exactly as
D-DISRUPT sets them; only the timer changes. Pair it with a dispatcher that
prefers to claim the next queued job for a tenant while the node is still warm
(it already polls at 500 ms after a job, so this comes for free).

Note the 2026-07-27 `Balanced` consolidation policy and the EKS 1.37 default
change (2026-10-01): new Auto Mode NodePools default to `Balanced`, not
`WhenEmptyOrUnderutilized`. The committed runner pools set `WhenEmpty`
explicitly, so they are unaffected; the `app` pool sets
`WhenEmptyOrUnderutilized` explicitly and is also unaffected.

### D/E. A real warm node, if you ever need one

If C still leaves a first-job-of-the-session cold start that matters, the
honest price of removing it is one idle `runner-sm` node: ≈ $95/mo on spot
(≈ $330 on-demand). Two ways to hold it:

- **Pause pod** with a negative `PriorityClass` (`-1000`,
  `preemptionPolicy: Never`) and the runner taint toleration, sized to one S/M
  pod. A real job preempts it, the placeholder goes Pending, Karpenter launches
  a replacement behind it. Scale the placeholder Deployment to zero at night or
  from the idle-suspend CronJob (§5.2) to stop paying when nobody is working.
- **Static capacity NodePool** (`spec.replicas: 1`, `limits.nodes: 2`): nodes
  are never consolidated and are replaced only for drift/expiry. Simpler than a
  pause pod but **one-way** (cannot switch back to dynamic) and it does not
  turn itself off; `kubectl scale nodepool --replicas=0` from the suspend
  CronJob would.

Either is the "KEDA/warm pod" idea from the question done the Kubernetes way.
Neither is needed for the cost-conscious target.

### Why the rest are no

- **Fargate.** 30–60 s per pod, image pulled every time, no instance-store
  NVMe (EBS-backed ephemeral capped at 175 GiB), no spot on EKS Fargate. It
  trades a 90 s node for a 45 s pod and loses the spill disk the L class needs.
- **KEDA.** An event-driven scaler that creates Jobs from a queue. The Postgres
  SKIP-LOCKED dispatcher already does that, and KEDA cannot make a node boot
  faster.
- **Nix.** Reproducible builds, not faster nodes. The runtime image is already
  `debian:bookworm-slim` plus static binaries; the gains in B come from
  shipping one DuckDB instead of three, which Nix would not change.
- **Snapshot-prebaked AMI.** Needs self-managed Karpenter (D-AUTOMODE), and the
  pull is the smaller term once B lands. Revisit only with the §18.16 number.

## 3. Recommendation

The perfect balance for a cost-conscious product is **no warm capacity, three
cheap changes**, in this order:

1. **Tiered dispatch (A).** S-class runs execute in-process on the always-on
   app node: zero startup, zero extra cost, and it is the path that exists
   today. Only M and L launch Jobs.
2. **Code-level startup fixes (B).** Bake the DuckLake extension and ship a
   single-binary runner image. Both are small PRs and pay on every run.
3. **`consolidateAfter: 10m` on `runner-sm` and `runner-l` (C).** One cold
   start per working session instead of one per job, for cents.

Expected result: S-class jobs start instantly; the first M/L job of a session
still pays ~60–90 s of node boot (which no AMI or image choice removes under
Auto Mode); everything after it in the next ten minutes starts in seconds.

Only if the first-M/L-job cold start is still unacceptable, add one pause pod
on `runner-sm` (D) and switch it off with the idle-suspend predicate. That is
the first option that costs real money (≈ $95/mo), so it should be the last one
reached for.

## 4. Measurements to take before implementing

- `kubectl get events` timestamps on one cold run: NodeClaim created → node
  Ready → image pulled → container started. This is §18.16 and it splits the
  90 s into launch / boot / pull / extension fetch.
- Pull time of the current `er-api` image on a fresh runner node versus a
  single-binary image.
- Wall time of an S-class `run_all_full` in-process on the `m6i.large` app node
  with 2 threads, so the in-process cap is set from a number.

## Sources

- EKS Auto Mode release notes: https://docs.aws.amazon.com/eks/latest/userguide/auto-change.html
- Static capacity NodePools: https://docs.aws.amazon.com/eks/latest/userguide/auto-static-capacity.html
- Cost optimization in EKS Auto Mode (bursty workloads): https://docs.aws.amazon.com/eks/latest/userguide/auto-cost-control.html
- Karpenter NodePools (`replicas`, `consolidateAfter`): https://karpenter.sh/docs/concepts/nodepools/
- Pod priority and over-provisioning: https://aws.amazon.com/blogs/containers/eliminate-kubernetes-node-scaling-lag-with-pod-priority-and-over-provisioning/
- Auto Mode June 2026 improvements (node boot −39%, image pull): https://aws.amazon.com/blogs/containers/faster-nodes-smarter-scaling-whats-new-inside-amazon-elastic-kubernetes-service-amazon-eks-auto-mode/
- SOCI parallel pull mode: https://aws.amazon.com/blogs/containers/introducing-seekable-oci-parallel-pull-mode-for-amazon-eks/
- Auto Mode under the hood (Bottlerocket): https://aws.amazon.com/blogs/containers/under-the-hood-amazon-eks-auto-mode
- EKS Fargate ephemeral storage: https://docs.aws.amazon.com/eks/latest/userguide/fargate-pod-configuration.html
- Karpenter provisioning time ranges (vendor): https://cast.ai/blog/eks-auto-mode/ , https://jorijn.com/en/knowledge-base/kubernetes/monitoring/karpenter-node-autoscaling-eks/
- c6id.2xlarge pricing (aggregator): https://www.devzero.io/instances/aws/c6id.2xlarge
