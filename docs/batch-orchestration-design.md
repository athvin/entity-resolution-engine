# Batch orchestration design: compiled DAGs, a task ledger, one Kubernetes executor

*The plan for running this repo's batch work — ingest, standardize, match, reconcile, assemble, train, correct, maintain, migrate, provision — as compiled directed acyclic graphs (DAGs) of tasks whose state lives in Postgres as Airflow-shaped task instances and whose only executor is a Kubernetes Job per task attempt. It marries the part of [dagr](https://github.com/athvin/dagr) we want (the graph is Rust and the compiler checks it; one binary is the pipeline) with the part of [Apache Airflow](https://github.com/apache/airflow) we want (a row per try, a cumulative retry budget, "clear one task" without re-running the run), and hands placement, capacity, deadlines and cleanup to the Kubernetes scheduler. Companions: [architecture.md](architecture.md) (entry point), [infrastructure.md](infrastructure.md) (the AWS/EKS substrate this executes on), [infrastructure-decisions.md](infrastructure-decisions.md) (the decisions this document amends by name), [entity-framework-design.md](entity-framework-design.md) (every table and statement below goes through it), [backend-design.md](backend-design.md) §5 (the Python-era job orchestration this supersedes). Written 2026-10-24; revised 2026-10-26 after review.*

---

## 1. Context and thesis

### 1.1 What we want

We run DuckDB on Kubernetes cheaply for many tenants. Every tenant's batch work is a chain of engine stages over that tenant's DuckLake, serialized to one writer per tenant. Today the chain runs as one in-process call inside the API pod ([dispatcher.rs](../rust/crates/er-server/src/dispatcher.rs) module header: "runs the matching `er-cli` op IN-PROCESS (no per-run k8s Jobs)"). A failure in the last stage re-runs the first; a deploy kills the run in flight; a 10M-record `match` is sized the same as the `golden` that follows it; and nothing in the Rust tree launches a Kubernetes object at all ([values.yaml](../infra/k8s/charts/er-platform/values.yaml) `dispatcher:` block: "NOT IMPLEMENTED … The workspace has no `kube` or `k8s-openapi` dependency").

dagr is the shape we like for the graph: `#[task]` derives a node's input and output from the `run` signature, handles only reference already-registered nodes so a cycle cannot be written, a mis-wired edge names both types in the compile error, and a non-default trigger rule on a data-dependent node is a type error (dagr `crates/macros/src/lib.rs`, `crates/core/src/binding.rs:347-361`, `crates/cli/src/flow_builder.rs:20-24`). What we do not like is that dagr carries two execution models — an in-process executor with admission pools and a remote one — because it is a laptop tool first (dagr `docs/arch.md:13`). We are all in on Kubernetes.

Airflow 3.4 is the shape we like for state: every try is its own `task_instance` row, only the latest is live, a cleared task re-bases its retry budget instead of resetting it, and the run goes back to queued while its successes stay succeeded (Airflow `airflow-core/src/airflow/models/taskinstance.py:1081-1097`, `:548`, `:431-651`). What we do not want is the general-purpose machinery around it: pools, priority weights, a triggerer, dynamic mapping, HA schedulers.

### 1.2 The answer in one paragraph

A **task is a pipeline stage**; a **DAG is a job kind**, written in Rust in the one multicall binary, with typed durable edges so that wiring, arity, cycles and trigger-rule misuse fail at `cargo build`. The **state of record is Postgres**: `jobs` stays the run (its id is still the run id), and a new `task_instances` entity carries Airflow's row-per-try with a live flag, a cumulative `max_tries`, and dagr's second budget for pods that never ran. The **only executor for stages is Kubernetes**: every pod task attempt is a `batch/v1 Job` with `backoffLimit: 0`, class-sized requests, runner-pool tolerations, `activeDeadlineSeconds` and `ttlSecondsAfterFinished`, and the pod re-enters the same binary as `er-server exec-task`. Control tasks (§3.6) are dispatcher-local Postgres steps under the same ledger, claim, retry and deadline discipline, with no engine code (`control_tasks_no_engine.rs`), no org slot and no resource model — which is what separates them from dagr's local executor, which runs stage bodies under admission pools. **Kubernetes owns** placement, node provisioning, bin-packing, spot, the deadline, cleanup and ordering under contention; **we keep** readiness over the compiled edges, the one-writer-per-tenant invariant (two partial unique indexes), class-based retry with two budgets, and the state of record. **We do not build** a second executor for stages, a pool table, a DSL, a workflow engine, or anything Kubernetes already does with a field.

### 1.3 What we take from each

| From dagr | From Airflow | From Kubernetes |
|---|---|---|
| The graph is code: typed handles, positional `Deps`, compile-time wiring and cycle refusal (dagr `crates/core/src/binding.rs`, `crates/cli/src/flow_builder.rs`) | One row per try with a live flag; archived rows stay in the table (`taskinstance.py:1081-1097`) | Placement, provisioning and bin-packing through `resources.requests`, `nodeSelector`, tolerations and the `karpenter.sh/v1` NodePools |
| Two hashes: a structural fingerprint that gates clear/resume and a policy hash that diverges with a diff and proceeds (dagr `crates/core/src/assembly.rs:581-589`, `crates/core/src/resume.rs:36-38`) | Cumulative `max_tries` re-based on clear: `max_tries = previous_try_number + retries` (`taskinstance.py:548`) | The attempt deadline (`activeDeadlineSeconds`) and cleanup (`ttlSecondsAfterFinished`), both independent of our process |
| The closed terminal taxonomy, three trigger rules and the can-never-fire table (dagr `docs/arch.md:27-56`, `crates/core/src/readiness.rs:26-51`) | Deterministic hash-jittered backoff in `[min, 2·min)` (`taskinstance.py:2064-2109`) | Ordering under contention through `PriorityClass` with `preemptionPolicy: Never` — runner pods never preempt; they stay preemptable by system-critical pods (§7.1) |
| Two retry budgets: a pod that never started costs a launch, not a try (dagr ADR 115 §6) | The stale-workload re-read before every create (`kubernetes_executor.py:479-546`) | Namespace ceilings through `ResourceQuota` and `LimitRange` |
| Pre-start and terminal classification that read both `reason` fields and never infer OOM from exit 137 (dagr `crates/k8s/src/executor.rs:26-56`) | `restarting` for clear-while-running; a finished run goes back to queued (`taskinstance.py:510-517`, `:598-637`) | Spot: `do-not-disrupt` against voluntary disruption; `DisruptionTarget.reason` with `status.reason` as the evidence for involuntary (§6.5) |
| The pod re-enters the same binary, refusing a mismatched fingerprint (dagr `crates/cli/src/exec_node.rs:248-279`) | `queued_by`/`external_executor_id`-style identity columns on the row | Owner-reference garbage collection of the pod by the Job |
| One watch per process with the 410/stall/backoff discipline; labels as selectors, annotations as authority; tombstones and two-step revocation (dagr `crates/k8s/src/observer.rs`, `identity.rs`, `adoption.rs`) | Zombie and stuck-in-queued detection as bounded sweeps (`scheduler_job_runner.py:3416-3586`, `:3939-4168`) | Logs and trace transport (Fluent Bit, the OTel collector) |
| **Deliberately not taken:** the local executor, admission pools, the event-stream fold and libSQL projection, the blob store, `inventory` registration, `satisfied-from-prior` and `abandoned` as states | **Deliberately not taken:** pools and `priority_weight`, the advisory-lock critical section, multi-executor routing, deferrable tasks and the triggerer, dynamic mapping, callbacks via the DAG processor, the Task Execution API as a separate service | **Deliberately not taken:** `podFailurePolicy: Ignore` (a cluster-side retry), `suspend` handshakes, `schedulingGates`, Indexed Jobs, topology spread, Argo Workflows, Kueue (deferred with a revisit trigger) |

## 2. Today, precisely

**The queue.** One Postgres table, [entities/job/entity.yaml](../entities/job/entity.yaml): `id` (UUIDv7), `org_id`, `kind`, `params`, `state`, `priority`, `idempotency_key`, `run_id`, `attempt`, `max_attempts` (default 3), `error_class`, `error_detail`, `not_before`, `created_by`, `schedule_id`, `resource_class`, `trace_context`, four timestamps. The declared states are `queued, dispatching, running, succeeded, failed, canceling, canceled`. Two invariants are indexes, not code: `jobs_idempotency` (unique `(org_id, idempotency_key)` where set) and `jobs_one_active_per_org` (unique `(org_id)` where `state IN ('dispatching','running','canceling')`).

**The claim** ([claim.sql](../entities/job/update/claim.sql)) is `UPDATE … SET state = 'dispatching', attempt = attempt + 1 WHERE id = (SELECT … NOT EXISTS (active job for the org) … FOR UPDATE SKIP LOCKED LIMIT 1)`; a lost race on the index maps to "nothing to claim" ([pg.rs](../rust/crates/er-server/src/pg.rs) `claim`). The attempt is counted at hand-out: "a worker that dies without reporting has still used one".

**The dispatcher** ([dispatcher.rs](../rust/crates/er-server/src/dispatcher.rs) `dispatch_once_with`) claims, marks `running`, reconciles the tenant's lake schema in `spawn_blocking` and records `tenant_schema_states`, then runs the kind in-process — a blocking `run_job` for the lake kinds, async arms for `provision`, `apply_corrections`, `apply_approved` and `train` — then marks `succeeded` or calls `fail_or_retry`, emits one `job.<state>` webhook per outcome, and sends the run report. It runs inside the merged `web-api` pod under `ER_DISPATCH_IN_PROCESS=1` ([deployment-web-api.yaml](../infra/k8s/charts/er-platform/templates/deployment-web-api.yaml)), `replicas: 1`, `strategy: Recreate`, one job at a time. Every job opens its session with `SessionOptions::deterministic()` ([er-cli/src/lib.rs](../rust/crates/er-cli/src/lib.rs) `open_lake`): one DuckDB thread, no memory limit, whatever the org's env says.

**Retry** is one statement, [fail_or_retry.sql](../entities/job/update/fail_or_retry.sql): `state = CASE WHEN $retryable AND attempt < max_attempts THEN 'queued' ELSE 'failed' END`, backoff `least(300, 5 · 2^(attempt−1))` seconds. Retryable means `pg::FailureClass::TransientIo`, decided by substring matching on the error string ([pg.rs](../rust/crates/er-server/src/pg.rs) `classify`: "open lake", "connection", "timed out" …). The engine's own taxonomy — `er_errors::ErrorClass` with seven classes and exit codes 0/1/2/3/10 ([er-errors/src/lib.rs](../rust/crates/er-errors/src/lib.rs)) — is "advisory to an operator; nothing acts on it".

**Resume** ([resume.sql](../entities/job/update/resume.sql)) sets a `failed|canceled` job back to `queued` and would append `resume_run_id` to `params` if `run_id` were set. It never is: no statement writes `jobs.run_id`, and nothing in `er-cli` or the dispatcher reads `resume_run_id`. The dispatcher uses the job id as the run id (`run_job`: `let run_id = &job.id`). A resume is therefore a from-scratch re-execution of the whole kind under the same run id, which duplicates every append-only ledger (§9.1).

**Cancel** ([lib.rs](../rust/crates/er-server/src/lib.rs) `cancel_job`) calls `mark(…, "canceled")` with no state precondition ([mark.sql](../entities/job/update/mark.sql) has no `WHERE state`), reaches no running thread, and is overwritten when the in-process work finishes. `canceling` is declared, is in the index predicate, and is never written. There is no reaper.

**Schedules** tick every 20 s ([dispatcher.rs](../rust/crates/er-server/src/dispatcher.rs) `SCHEDULE_TICK_EVERY`), enqueue with idempotency key `sched:{schedule_id}:{minute}`, no catch-up; `SCHEDULABLE_KINDS` ([lib.rs](../rust/crates/er-server/src/lib.rs)) is `run_all_full, run_all_incremental, correct, train, lake_maintain, apply_approved`; `tests/dispatch_coverage.rs` scans `dispatcher.rs` for a string-literal arm per kind.

**One writer per tenant** is the `jobs_one_active_per_org` index and nothing else: there is no advisory lock on the lake side in the Rust tree, and [architecture.md](architecture.md) line 82 ("A PostgreSQL advisory lock permits one writer per tenant") describes the deleted Python `tenant_lock`. The [er-engine-ops skill](../.claude/skills/er-engine-ops/SKILL.md) states it correctly: the guarantee "comes from the control plane's queue, not from the CLI".

**The measured cold start** ([measurements/runner-startup-20261024.json](measurements/runner-startup-20261024.json), summarised in [infrastructure.md](infrastructure.md) §18.16): NodeClaim created → Ready **14–18 s**; scheduled → pulling ~3 s; image pull **1.6–3.4 s** (109.1 MB compressed, extensions baked); first session **0.9–1.4 s**; submit → container running **~30 s**. With `consolidateAfter: 10m` ([runner-sm.yaml](../infra/k8s/nodepools/runner-sm.yaml)) a second pod on the warm node scheduled → started in ~1 s with no pull. The account's Standard vCPU quotas are both **8**, app and system nodes hold 4, so **no runner node of either class can be provisioned today** (`runner-sm.yaml` comment; [infrastructure.md](infrastructure.md) §5).

**Recorded decisions this plan touches**, each amended by name where it is amended:

| Decision | Recorded position | This plan |
|---|---|---|
| D-ARGO | one native Job per run; "revisit when a run ever becomes a genuine multi-pod DAG" | reaffirmed in substance, amended in context: the run is now a multi-pod DAG and Argo is still rejected for the same two-sources-of-truth reason (§7.4) |
| D-JOBSPEC | `backoffLimit: 0`, `restartPolicy: Never`, EFS mount — nothing else; the TTL 3600 and the class deadlines S 2 h / M 6 h / L 24 h are [infrastructure.md](infrastructure.md) §6.2's Job sketch, not the decision | amended: Job per **task attempt**, TTL 300, `limits.cpu` set, termination message, no `podFailurePolicy` (§6.2); the 120 s termination grace is set here (§5.6) |
| D-PROGRESS | the runner writes its own progress to the control-plane DB | amended: the runner still originates progress, through `er-server`'s attempt channel, never with a DSN (§6.4) |
| D-SIZING | class per **run** on `jobs.resource_class` | amended: class per **task**, OOM escalation S→M→L (§5.5, §8) |
| D-DISRUPT | `WhenEmpty`, `do-not-disrupt`, `consolidateAfter` 30 s (live 10m) | unchanged; the 10m linger is what makes per-task pods cheap (§8) |
| D-SPILL | ephemeral-storage request + `emptyDir.sizeLimit` per class | unchanged, now per task (§6.2) |
| D-TENANCY | one platform namespace; "the Job spec anticipates a ServiceAccount parameter" | realised: the parameter is `er-runner` (§6.7) |

## 3. The unit of work and the compiled DAG

### 3.1 A task is a stage

`run_all_ensemble` ([er-cli/src/lib.rs](../rust/crates/er-cli/src/lib.rs) from line 3188) is already a DAG written as a function: run row → lexicon temp table → ingest per delivery → standardize → match (then `screen_deliveries`) → reconcile → golden → reviews → metrics/quality → run end. Every boundary is a committed DuckLake snapshot; the only in-process state is the `nickname_variants` temp table and the session. That is dagr's "durable, addressable output" at every edge (dagr `crates/core/src/resume.rs:60-68`) with no work. Sub-stage granularity would force the temp catalog's intermediates through the lake for no retry benefit; whole-run granularity is today's, and is why a transient failure in `golden` re-runs `match`. Stage granularity is also where sizing differs: `match` is the memory and thread consumer ([infrastructure.md](infrastructure.md) §6.5), `ingest` and `assemble` are not.

The task vocabulary follows `RUN_STAGES` ([er-schema/src/model.rs](../rust/crates/er-schema/src/model.rs)), today nine — `init, ingest, standardize, train, match, reconcile, assemble, maintain, reset` (`assemble` is golden + reviews + metrics + quality) — plus two new stages added to that constant and its golden, `finish` and `apply_staged`, which moves the `assert_eq!(RUN_STAGES.len(), 9)` pin to 11 (§4.3). `screen_deliveries` is the tail of `match`. Tasks that run with no run row (`reconcile_schema`, `lake_init`, every control task) write no lake-ledger row. Fan-out inside a stage (N deliveries) is the task looping internally; the graph's shape never changes at runtime (dagr `docs/arch.md:19`).

**Rejected alternative.** One task per delivery for `ingest`. It would make the graph's shape a function of the drop directory, which is exactly the dynamic mapping Airflow needs `dag_version` and regions for and dagr refuses; the receipt per `(run_id, source)` already gives per-delivery idempotency inside one task (§9.1).

### 3.2 The authoring surface

A new crate `rust/crates/er-dag` holds the traits, the builder, readiness, the fingerprints and the resume algorithm. It depends on `er-errors`, `er-ids` and `serde` only — no kube, no Postgres, no engine. The DAG definitions and task bodies live in the leaf binary, `er-server/src/dags/`, because that one binary is the dispatcher, the API and the pod (dagr's own rule that the registration must live in the leaf binary, `docs/arch.md:559`). Pod task bodies call `er_cli::*` stage functions; control task bodies call `pg::*`.

```rust
// er-dag/src/task.rs
pub trait Task<S>: Default + 'static {               // S: the lake session type the leaf binary supplies
    /// Author-declared stable identity, recorded on the row and the pod. A Rust rename does not move it.
    const NAME: &'static str;
    /// Feeds the structural fingerprint. A test pins it to a hash of the task's declared statement set (§9.4).
    const VERSION: u32;
    /// Positional inputs: () | (A,) | (A, B) … up to 8, each a DurableOutput. Sealed.
    type Input: Deps;
    /// Always a reference, never a value: an in-memory edge does not type-check.
    type Output: DurableOutput;
    /// True for every task that writes the tenant lake. Feeds the assembly single-writer check.
    const WRITES_LAKE: bool;
    fn policy() -> NodePolicy { NodePolicy::default() }
    /// Runs inside the pod. `&mut self` is what makes "try n+1 never overlaps try n" free.
    fn run(&mut self, ctx: &mut TaskCtx<'_, S>, input: Self::Input) -> Result<Outcome<Self::Output>, TaskError>;
}

/// What a pod task body receives. er-dag defines it; er-server's verb fills it.
pub struct TaskCtx<'a, S> {
    pub order: &'a WorkOrder,     // run/job/task/try identity, RunPlan, params, class, segment bounds, tenant lake env
    pub session: &'a S,           // the open lake session (dedupe_core::Session in the leaf binary; er-dag never names it)
    pub counters: &'a CounterSink, // replaced on every heartbeat and at finish
    pub fence: &'a AtomicBool,    // set by the heartbeat thread; checked before the last write-back (§6.3 step 6)
    pub cancel: &'a CancelToken,  // §5.6; the body may poll it between its own statements
}

/// A control task (§3.6): the Postgres half of a kind, run inside the dispatcher. `C` is the control-plane client type.
pub trait ControlTask<C>: Default + 'static {
    const NAME: &'static str;
    const VERSION: u32;
    type Input: Deps;
    type Output: PlanOutput;      // a `Plan<P>` only: assembly refuses a `LakeRef` output on a control task
    fn policy() -> NodePolicy { NodePolicy::control() }   // retries 2, backoff 5 s / 60 s, timeout 60 s, no class, no segment
    fn run(&mut self, ctx: &mut ControlCtx<'_, C>, input: Self::Input) -> Result<Outcome<Self::Output>, TaskError>;
}

pub enum Outcome<O> { Done(O, Counters), NoOp(O) }   // NoOp is exit 10: a success with outcome = no_op
pub enum TaskError { Failed(er_errors::ErError), Skip } // class comes from ErError; Skip is the task branching

/// A set of lake relations at the DuckLake snapshot the producer committed. The only lake edge.
pub struct LakeRef<E: LakeEdge> { pub snapshot_end: i64, pub rows: i64, _e: PhantomData<E> }
pub trait LakeEdge: 'static { const STABLE_NAME: &'static str; const RELATIONS: &'static [&'static str]; }
/// A small typed document the control plane stores on the producer's row (`task_instances.output`).
pub struct Plan<P: Serialize + DeserializeOwned + 'static>(pub P);

pub trait DurableOutput: Sized + 'static {
    const STABLE_NAME: &'static str;
    fn serialize_reference(&self) -> serde_json::Value;
    fn rehydrate(v: &serde_json::Value) -> Result<Self, RefError>;
    /// Existence probe. For LakeRef: does DuckLake still hold `snapshot_end`? Catalog introspection over
    /// `ducklake_snapshots`, never a relation read; `Missing` once `lake_maintain`'s `expire_snapshots` removed it.
    fn probe(&self, prober: &dyn Prober) -> Result<ReferenceExistence, RefError>;
}
/// Supplied by the leaf binary, which owns the lake `Session`; er-dag never names the engine.
pub trait Prober { fn snapshot_exists(&self, snapshot: i64) -> Result<bool, RefError>; }
pub enum ReferenceExistence { Present, Missing }

pub struct NodePolicy {
    pub retries: u32,                      // tries beyond the first; default 2 (= jobs.max_attempts 3 today)
    pub backoff: Backoff,                  // base 30 s, factor 2, cap 900 s; deterministic jitter in [min, 2·min)
    pub timeout: Option<Duration>,         // -> Job.activeDeadlineSeconds; None -> class default
    pub class: ClassRule,                  // Fixed(S|M|L) | FromRun (jobs.resource_class, today written as None at both enqueue sites)
    pub escalate_on_oom: bool,             // default true for match and reconcile
    pub segment: Option<&'static str>,     // fusion segment (§3.7); policy hash only
    pub teardown: bool,                    // runs under a canceling job too (finish)
}
```

The two session parameters are how `er-dag` keeps its dependency set: the leaf binary fixes `S = dedupe_core::Session` and `C = tokio_postgres::Client`, so no engine or driver type is named in the crate. The builder has two registration methods — `d.then::<T>(deps)` for a `Task<S>` and `d.control::<T>(deps)` for a `ControlTask<C>` — and the node records which it was, which is what `plan.sql` writes as `kind`; the `epilogue` form exists for both.

`Prober` is implemented once, in `er-server/src/dags/` beside the pod task bodies (never under `dags/control/`, §3.3), by wrapping a lake `Session` through a new `Session::snapshot_exists(version)` in dedupe-core — `SELECT count(*) FROM ducklake_snapshots('<alias>') WHERE snapshot_id = ?`, a bound parameter, beside `latest_snapshot_version`/`list_snapshots` ([dedupe-core/src/session/mod.rs](../rust/crates/dedupe-core/src/session/mod.rs)). That is the framework's recorded catalog-introspection exemption, not a persistent lake read, so `freeform_sql.rs`, `declared_names.rs` and E006 are untouched and no `lake.main.<rel>` text is ever assembled at runtime; the exemption lists in the [entity-framework skill](../.claude/skills/entity-framework/SKILL.md) and [entity-framework-design.md](entity-framework-design.md) ("The three governance categories") gain the method. Both probe sites — §6.3 step 5 and the clear handler (§5.4) — go through it. `ReferenceExistence` has no `Changed`: a snapshot is immutable, so a pinned count could only return the recorded `rows` or fail; the head-moved case is the lake-head check of §9.2 (2). The edge relations are `Landed` {`raw_records`, `ingest_batches`}, `Standardized` {`int_std_records`}, `Scored` {`match_scores`, `scored_records`}, `Resolved` {`entities`, `entity_membership`, `entity_events`}, `Assembled` {`golden_records`, `review_queue`, `metric_snapshots`, `metric_snapshot_sources`}; `tests/dag_assembly.rs` asserts every `LakeEdge::RELATIONS` entry names a relation in `er_schema::model::all_specs()`, so an unknown relation is an assembly refusal, mirroring `declared_names.rs`.

**Rejected alternative.** One declared `read/count.sql` per edge relation — `SELECT count(*) AS n FROM lake.main.<rel> AT (VERSION => $snapshot)`, the shape `golden_record` and `int_std_record` already carry — dispatched through a closed `match` on the relation name. It is gate-legal (a declared, E006-pinned read called as `er_entities::<entity>::read::count`, the path `read.rs` already uses), and it would also prove the relation readable at that version and cross-check `rows` against the ledger, but it costs a statement pair per relation for a question — does snapshot *S* still exist — that is relation-agnostic DuckLake metadata.

```rust
// er-server/src/dags/run_all.rs
pub fn run_all() -> Dag {
    let mut d = Dag::new(JobKind::RunAllFull.dag_name());
    let plan   = d.source::<Init>();                           // Handle<Plan<RunPlan>>
    let raw    = d.then::<Ingest>((plan,));                    // Handle<LakeRef<Landed>>
    let std    = d.then::<Standardize>((plan, raw));           // Handle<LakeRef<Standardized>>
    let scored = d.then::<Match>((plan, std));                 // Handle<LakeRef<Scored>>
    let ents   = d.then::<Reconcile>((plan, scored));          // Handle<LakeRef<Resolved>>
    let done   = d.then::<Assemble>((plan, ents));             // Handle<LakeRef<Assembled>>
    d.epilogue::<Finish>(&[done.id()], TriggerRule::AllTerminal); // Finish: Task<Input = ()>; writes runs.status
    d.build()
}

// er-server/src/dags/mod.rs — the registry. Exhaustive: a kind without a DAG does not compile.
pub fn dag_for(kind: JobKind) -> Dag {
    match kind {
        JobKind::RunAllFull | JobKind::RunAllIncremental => run_all(),
        JobKind::Correct          => correct(),
        JobKind::Train            => train(),
        JobKind::LakeMaintain     => lake_maintain(),
        JobKind::LakeMigrate      => lake_migrate(),
        JobKind::Provision        => provision(),
        JobKind::ApplyCorrections => apply_corrections(),
        JobKind::ApplyApproved    => apply_approved(),
    }
}
```

`JobKind` is an enum with `as_str()`/`FromStr` over the exact `jobs.kind` strings and a `const ALL`; `SCHEDULABLE_KINDS` becomes a subset asserted against `ALL` in a test. A `jobs.kind` that does not parse is a terminal `config` failure at planning, named in `error_detail`.

**Rejected alternative.** dagr's `inventory`-based `#[dag]` registration (k8s-native design). It is link-time, cross-crate-unsupported by dagr's own account, and gives no compile-time coverage; an exhaustive `match` does, and leaves `dag_for` as a literal for a source-scanning gate to find (§11, Phase 1 rewrites `dispatch_coverage.rs` to scan it).

### 3.3 What is checked, and when

**rustc.** `d.then::<T>(deps)` requires `deps: Deps<T::Input>` — the tuple of handles must match the task's declared inputs positionally, and a mismatch names both types. Handles are minted only by the builder, so an edge can only point at an already-registered node: a cycle is unwritable. `Output: DurableOutput` means an in-memory type cannot be an edge. `epilogue::<T>` is bounded `T: Task<Input = ()>`, so a non-default trigger rule on a data-dependent node is a type error — dagr's typestate as a trait bound. Arity is sealed at 8. Control tasks implement `ControlTask` (§3.6), whose `run` takes a Postgres client and no `Session`.

**Assembly** (pure, at startup and in `tests/dag_assembly.rs`): duplicate names, an empty graph, a segment that is not contiguous or spans two classes, `escalate_on_oom` on a `Fixed(L)` task, a `teardown` task with data inputs, and — new for this product — **two `WRITES_LAKE` tasks with no path between them**, which would be two concurrent writers of one tenant lake. That check is the graph-level image of `jobs_one_active_per_org`; the DB-level image is `task_instances_one_pod_per_org` (§4.6). Assembly also computes both hashes and the topological `seq` of every node.

**Startup** (`er-server` boot and the pod verb): `dag_for` is evaluated for every `JobKind::ALL` and assembled; a problem is a refusal to start, not a log line. A textual gate `tests/control_tasks_no_engine.rs` fails if any file under `er-server/src/dags/control/` names `er_cli::`, `dedupe_core::` or `open_lake` — the same structural-claim-by-source-scan the repo already uses in `tests/egress_guard.rs` and `tests/dispatch_coverage.rs`.

### 3.4 Two hashes

`structural_fingerprint = sha256(dag name; for each node in seq order: NAME, VERSION, input STABLE_NAMEs, output STABLE_NAME, WRITES_LAKE, trigger rule, teardown; edges)`. `policy_hash = sha256(per node: retries, backoff, timeout, class rule, escalate_on_oom, segment)`. Both are written on the job row at planning and stamped on every Job as annotations. A clear or an adoption compares the structural fingerprint and refuses with the per-node diff on mismatch (HTTP 409; the only offered action is a new job). A policy divergence proceeds with the diff written to `audit_log` — "raising a timeout and resuming the expensive half-finished run is the motivating case" (dagr `crates/core/src/resume.rs:36-38`). dagr's honest limit stands: a task body change moves neither hash, so `VERSION` is a hand-maintained marker — pinned in a test to the hash of the task's declared statement set (§9.4) so the common body change, a different SQL statement, cannot be forgotten.

### 3.5 Kinds → DAGs

| Kind | Tasks (pod unless marked control) | Notes |
|---|---|---|
| `run_all_full`, `run_all_incremental` | `init → ingest → standardize → match → reconcile → assemble`; `finish` (`all_terminal`, teardown) | one DAG, two kinds; mode is a run parameter `init` resolves through the drift guard (`allow_escalate`) and writes into `RunPlan` |
| `correct` | `init → reconcile → assemble`; `finish` | `init` pins the latest live model identity (`run/read/latest_live.sql`) into the plan |
| `train` | `reconcile_schema → train → register_model` (control) | no run row today and none added; `train` writes `models/<run_id>/{model.json,tf.csv}` on EFS; `register_model` creates the draft `trained-{run_id}` row pointing at that per-run TF path — the shared-CSV append at `append_tf_snapshot` is deleted (§9.1) |
| `lake_maintain` | `reconcile_schema → maintain` | class from lake size; `maintenance` priority |
| `lake_migrate` | `reconcile_schema` | the reconcile is the work; `tenant_schema_states` written through the channel exactly as today |
| `provision` | `create_catalog` (control) → `lake_init` → `activate` (control) | `create_catalog` creates the tenant database on the maintenance DSN or the local lake dir; `lake_init` creates and reconciles the lake (today's `reconcile_lake_env` in `provision_tenant`); `activate` is `set_org_state(provisioning → active)` |
| `apply_corrections` | `drain_actions` (control) → `init → apply_staged → reconcile → assemble` → `settle_actions` (control); `finish` | `drain_actions` snapshots the pending staged-action ids as `Plan<DrainedActions>`, `init`'s upstream; `settle_actions` marks applied, writes labels, emits `review.resolved`. `entity.write_back` is **not** in the control body: its payload needs a lake open (`write_back_resolutions`), so it is pod-side work — open item in §12 |
| `apply_approved` | `stage_approved` (control) | reads tags, stages actions, enqueues `apply_corrections`; never opens a lake |

`init` is new and load-bearing: it opens the lake, runs the schema reconcile once (the pre-step leaves the dispatcher, which after Phase 4 opens no lake at all), reports the tenant schema state through the channel, makes the drift-guard mode decision once, deletes-then-inserts the `runs` row (§9.1), and emits `Plan<RunPlan { mode, rebuild_reason, model_version, tf_snapshot_id, lexicon_hash, snapshot_start, run_at }>`, which every downstream task consumes. `match` re-asserts the plan's fingerprint equals the live one inside its own transaction and fails `precondition` otherwise, so a run that straddles a config publish refuses rather than mixes (§9.5). `reconcile_schema` is the reconcile half of `init` for kinds with no run row.

`provision` could not stay a pure control task: `provision_tenant` runs `reconcile_lake_env` on the new lake ([dispatcher.rs](../rust/crates/er-server/src/dispatcher.rs) `provision_tenant`), which opens it, so by the rule that anything opening a lake is a pod task it is split as above; `create_local_lake` itself only creates directories and stays in the control body.

### 3.6 Control tasks

A control task is a `task_instances` row with `kind = 'control'` whose body runs inside the dispatcher process with the control plane's own credentials, under the same ledger discipline as a pod task. It exists for the Postgres halves of kinds that already have them — model registration after `train`, draining and settling staged actions around `apply_staged`, the org state transition after `provision` — and makes them visible, idempotent and retryable; the registry refuses a control task that links engine code (§3.3), so it structurally cannot run a stage. Control tasks hold no org slot (`task_instances_one_pod_per_org` is scoped to `kind = 'pod'`), never count toward `executor.inFlightCap`, and never leave the dispatcher process.

**Execution contract.** Three commits, so a crash between them is recoverable — not one transaction: `claim_control.sql` (`scheduled | up_for_retry → running`, committed before the body runs); the body on the dispatcher's own connection under `SET LOCAL statement_timeout` and `tokio::time::timeout(NodePolicy::timeout)` — the control analogue of `activeDeadlineSeconds`; expiry is `timeout`, terminal per §5.5; then `finish_control.sql` or `fail_or_retry` (§4.5). No heartbeat; §5.7's zombie sweep is scoped `kind = 'pod'`. Every control body is idempotent under `(job_id, task_id)` because it may re-run after a crash between its own commit and `finish_control` — §9.2's contract extended. Per body: `create_catalog` is explicitly **non-transactional** — `CREATE DATABASE` autocommits on the maintenance DSN and cannot run inside a transaction block, `create_workspace` writes EFS — with the `pg_database` probe, `config_path.exists()` and `create_dir_all` as its idempotency keys, exactly as `create_tenant_database` probes today; `activate` treats `set_org_state` returning false with the state already `active` as success; `register_model` probes the registry for label `trained-{run_id}` before `create_model_version` (the table is unique on `(org_id, name, version)`, not label); `drain_actions` emits its snapshot of pending action ids as its own output (`Plan<DrainedActions>`), which `init` consumes as a positional upstream — a re-run after a crash re-snapshots, which is correct because nothing downstream has run; `stage_approved` keeps its per-review tag-removal idempotency; `settle_actions` runs `mark_action_applied`, `record_reviewer_label` and the `review.resolved` outbox inserts in the same transaction as `finish_control` (all Postgres rows). A pure-Postgres body *may* be one transaction with its finish; that is a property some bodies have, not the contract. Adoption (§6.6): a live `running` control row is orphaned by construction (`replicas: 1`, `Recreate`, the SIGTERM drain finishes the in-flight body), so the new epoch runs `fail_or_retry` on it at once with `error_class = infrastructure`, `termination_reason = dispatcher_restart`. The launcher loop alternates `claim_launch` (pod) and `claim_control` (control).

**Rejected alternatives.** Prologue/epilogue steps that are not tasks (dagr-first design): invisible in the UI, cannot be retried alone, and recreate the untracked Postgres halves `dispatch_once_with` has today. Running control bodies in pods: the pod holds no `ER_PG_DSN` (§6.4) and the maintenance DSN is more privileged still, at 3–30 s per step for milliseconds of SQL.

### 3.7 Fusion into segments

Per-task pods cost ~3 s each on a warm node (§8). `NodePolicy::segment` groups **contiguous, same-class** tasks into one pod. The launcher claims and launches only the segment's first non-succeeded member (`claim_launch` is unchanged: `LIMIT 1`, one Job, `--attempt <head> --through <last>`); every later member stays `none` until the pod reaches it. The verb runs the chain in one process and DuckDB session (the lexicon temp table is seeded once; intermediates stay in the temp catalog) and reports each task boundary through the channel: the pod's `finish` for member *k* carries `next = k+1`, and the handler runs `finish.sql` for *k* and `advance_segment.sql` for *k+1* (`none → running`, copying the Job's identity from *k*'s row, §4.5) in **one Postgres transaction**, finish first. So there is never an instant with *k* terminal and *k+1* unclaimed; the one-pod index and the in-flight cap each see exactly one live pod row per segment at every statement boundary (a partial unique index is checked per statement, which [entities/matching_model/entity.yaml](../entities/matching_model/entity.yaml) already relies on); and the dispatcher's readiness pass cannot race the pod — `mark_scheduled` requires `state = 'none'`, which the advance has already left, so the pass only confirms. Every task keeps its own row, `run_stages` row, snapshot range and verdict; the segment's Job carries the sum of its members' timeouts as `activeDeadlineSeconds`. If task *k* fails, tasks before *k* stay `succeeded`, the pod exits, and the retry launches the segment from *k*: *k*'s new try row is claimed by `claim_launch` as the head of the remaining segment and its Job name derives from *k*'s id. Segments are computed at plan time from resolved classes and re-derived by the launcher's pass whenever a rotation changed a class: an OOM escalation inserts its successor with `segment = NULL` (§4.5) and a clear may carry a class override, and in both cases the pass before the next launch recomputes the affected job's segments from its live rows. Default: an S or M run is one segment — today's shape and latency, with per-stage retry for free; an L run splits at the class boundary into `[init, ingest, standardize]`@M, `[match]`@L, `[reconcile, assemble, finish]`@M. Fusion is in the policy hash, so changing it never refuses a resume.

**Class resolution at planning.** `ClassRule::Fixed` is final. `ClassRule::FromRun` resolves, in order: `jobs.resource_class` when the enqueue set it (an operator override through `POST /jobs`, or the `resource_class` a clear passed); else the tenant's last succeeded `init` output — `RunPlan.corpus_records`, which `init` reads through `relation_row_count` (the catalog-introspection exemption) and emits, read back from `task_instances.output` of the org's latest succeeded `init` row — mapped through the D-SIZING thresholds (S ≤ 100k, M ≤ 1M, else L); else **M**, the default before any run has completed ([backend-design.md](backend-design.md) §7). `lake_maintain`'s "class from lake size" is the same lookup. The planner records the resolved class on every row and derives segments from it; nothing re-resolves mid-run.

**Rejected alternatives.** Dynamic lane pull — the pod claims the next ready task in its lane through a `/next` route (k8s-native design): it races the launcher and needs a "skip if the lane pod finished less than 10 s ago" heuristic; a plan-time segment is deterministic, race-free and legible in the row. Pre-claiming every member to `queued` under one Job name at launch: it collides with `task_instances_one_pod_per_org`, counts a six-task segment as six against the cap, and orphans the siblings in `queued` when the head fails. Dropping fusion: it spends the §8 latency argument on a missing transition statement; it stays the fallback only if the Phase 2 double shows the finish+advance transaction cannot be made atomic on the per-request connection.

## 4. The data model as entities

Every table below is an `entities/*/entity.yaml` and every statement a declared `.sql` + `.yaml` pair; `er-entity gen` emits the structs, DDL, TypeScript bindings and typed functions, and the new tables land at boot through the baseline replay ([pg.rs](../rust/crates/er-server/src/pg.rs) `ensure_schema`). Rules the sketches honour: every control write is `mode: partial`, `returns: entity` with the full RETURNING list (abbreviated `RETURNING <all columns>` here for length; E003 requires every column in declaration order), every UPDATE sets `updated_at = now()` (E014), closed sets are `accepted_values` on day one because a CHECK added later never lands on an existing table, and new columns on `jobs` and `task_instances` are nullable because a NOT NULL column absent from a live table is Breaking for the reconciler ([er-entity-diff](../rust/crates/er-entity-diff/src/lib.rs)). The framework has no multi-statement form, and E012 accepts a `WITH … INSERT` (which sqlparser reads as `Statement::Query`) in no write folder, so a decision that archives one row and inserts its successor is **two declared files run in one transaction** on the per-request connection (`client.transaction()`, the shape [pg.rs](../rust/crates/er-server/src/pg.rs) already uses); `pg_task_instances.rs` pins that shape, not "either shape". No `history: true` on the task table, for the claim/heartbeat-churn reason `job` has none ([entity-framework-design.md](entity-framework-design.md) "Identity conventions"); the archived tries are the history.

### 4.1 `jobs` is the run

`job.id == run_id` is load-bearing everywhere (`run_job`, the webhook `detail_path`, `metric_snapshots.run_id`, `entity_events.run_id`), so we extend rather than add. New nullable columns: `structural_fingerprint`, `policy_hash`, `image_digest`, `clear_number` (bigint; read as `COALESCE(clear_number, 0)`), `owner_epoch` (one reader: `job/update/adopt.sql`, §6.6). Planning finally writes `run_id = id`, in the same transaction as the claim (§4.5), so `dispatching` is never a committed state on the DAG path. `jobs.attempt`/`max_attempts`/`fail_or_retry.sql` keep their meaning for the legacy arm during the rollout (§11); after Phase 4 a planning failure is terminal (`config` or `internal`), the retry branch is deleted and `max_attempts` is frozen at its default. `attempt` is not frozen: `claim.sql` increments it on every claim, including the re-claim after a clear (§4.5), so it becomes the **claim counter** and the job page's "Attempt {attempt} of {max_attempts}" panel ([job-detail.tsx](../frontend/src/components/runs/job-detail.tsx)) moves to the task list's `try/max_tries` (§10). The dead `resume_run_id` branch in `resume.sql` is deleted with the legacy arm. States and the `jobs_one_active_per_org` index are unchanged; `canceling` gains a writer (§5.6).

### 4.2 `entities/task_instance/entity.yaml`

```yaml
kind: entity
version: 1
name: task_instance
relation: task_instances
description: "One try of one task of one job (Airflow's task_instance shape): the live try has live = TRUE, archived tries have live NULL and stay in the table as history."
key: [id]
natural_key: [job_id, task_id, try_number]
generate: { stores: [postgres], languages: [rust, typescript], wire: [json] }
columns:
  - { name: id, type: varchar, nullable: false, description: "er_ids::natural_id('task_instances', [job_id, task_id, try_number]): a pure function of the attempt key, so the Job name is one too." }
  - { name: org_id, type: varchar, nullable: false, references: { relation: orgs, column: id }, description: "Tenant." }
  - { name: job_id, type: varchar, nullable: false, references: { relation: jobs, column: id }, description: "The run (jobs.id == run_id)." }
  - { name: task_id, type: varchar, nullable: false, description: "Task::NAME, stable across renames." }
  - { name: seq, type: bigint, nullable: false, description: "Topological position from assembly; display and claim order." }
  - { name: try_number, type: bigint, nullable: false, default: "1", description: "1-based try. A new row per try." }
  - { name: live, type: boolean, description: "TRUE on the live try, NULL once archived (NULLs are distinct, so the partial unique index below admits one live row per coordinate)." }
  - { name: archived_reason, type: varchar, accepted_values: [retry, cleared, restarted], description: "Why the row stopped being live; NULL while live. retry = archive_try, cleared = a terminal row rotated by clear, restarted = a clear-while-live rotation (complete_restart). Pending rows reset by a clear are not archived (no try was consumed)." }
  - { name: kind, type: varchar, nullable: false, default: "'pod'", accepted_values: [pod, control], description: "Where the body runs: a Kubernetes Job, or a dispatcher-local body under §3.6's contract." }
  - { name: state, type: varchar, nullable: false, default: "'none'",
      accepted_values: [none, scheduled, queued, running, up_for_retry, restarting, canceling, succeeded, skipped, failed, timed_out, upstream_failed, upstream_skipped, canceled],
      description: "Closed state set (§5). restarting and canceling are the write-ahead markers for clear and cancel of a live row." }
  - { name: trigger_rule, type: varchar, nullable: false, default: "'all_succeeded'", accepted_values: [all_succeeded, all_terminal, any_failed], description: "Copied from the compiled graph at planning so the UI can explain propagation; the graph is the authority." }
  - { name: upstream, type: json, nullable: false, default: "'[]'::jsonb", description: "Upstream task_ids, a UI projection of the compiled edges." }
  - { name: teardown, type: boolean, nullable: false, default: "false", description: "Claimable while the job is canceling (finish)." }
  - { name: segment, type: varchar, description: "Fusion segment id resolved at planning; NULL for a task that runs alone." }
  - { name: resource_class, type: varchar, accepted_values: [S, M, L], description: "Class this try runs at; NULL for a control task. OOM escalation writes a larger class on the next try." }
  - { name: next_resource_class, type: varchar, accepted_values: [S, M, L], description: "Operator override recorded by clear on a restarting row; complete_restart applies it to the successor. Additive, so the boot reconciler applies it." }
  - { name: escalated_from, type: varchar, accepted_values: [S, M], description: "Class of the OOMKilled predecessor try, so a mixed-class run is legible." }
  - { name: max_tries, type: bigint, nullable: false, default: "0", description: "Cumulative try budget (Airflow max_tries): retries at planning; previous_try_number + retries after a clear, where previous_try_number is try_number for a rotated terminal row and try_number - 1 for a pending row re-gated in place. Eligible to retry while try_number <= max_tries." }
  - { name: launches, type: bigint, nullable: false, default: "0", description: "Infrastructure budget consumed by this try: Job creates whose pod never ran user code, plus disruptions while running; 1 for a member advanced inside a running segment pod." }
  - { name: max_launches, type: bigint, nullable: false, default: "3", description: "Infrastructure budget per try." }
  - { name: not_before, type: timestamp, description: "Backoff deadline for up_for_retry; a 30 s capacity wait for scheduled." }
  - { name: input, type: json, description: "The work order: pinned upstream references, the RunPlan, params. Written whole by mark_scheduled at every readiness verdict (never merged), by advance_segment for a fused member, and copied by fail_or_retry; NULL on a none row. Read by the pod through the channel." }
  - { name: output, type: json, description: "The serialized DurableOutput (a LakeRef or Plan), written at finish." }
  - { name: outcome, type: varchar, accepted_values: [done, no_op], description: "Success flavour; no_op is exit 10." }
  - { name: exit_code, type: bigint, description: "Container exit code: the pod's own on finish, the watch's observation on record_evidence. Evidence for the row, never a classifier input (§6.5)." }
  - { name: termination_reason, type: varchar, description: "Platform evidence, distinct from error_class, free text: OOMKilled, Evicted, NodeLost, the DisruptionTarget reason (TerminationByKubelet, EvictionByEvictionAPI, PreemptionByScheduler, DeletionByTaintManager, DeletionByPodGC), DeadlineExceeded, Unschedulable, ImagePullBackOff, ErrImagePull, CreateContainerConfigError, InvalidImageName, FailedCreate, no_pod_created, vanished, quota, sigterm, sigkill, fenced, stale_start, structural_mismatch, build_skew, control_plane_unreachable, no_terminal_record, heartbeat_timeout, dispatcher_restart, revoked, …" }
  - { name: error_class, type: varchar, accepted_values: [transient_io, lock_conflict, precondition, config, contradiction, non_convergence, data, internal, infrastructure, resources, timeout], description: "er-errors' seven classes plus the executor's four." }
  - { name: error_detail, type: varchar, description: "Chain-flattened text (errfmt::chain), truncated to 2000 chars." }
  - { name: owner_epoch, type: varchar, description: "The dispatcher process epoch (UUIDv7 minted at start) that claimed the launch; adoption rewrites it." }
  - { name: k8s_job_name, type: varchar, description: "batch/v1 Job name, er-t-<20 hex of id>-l<launches>, of the Job that ran this try; for a fused non-head member, the segment head's Job (advance_segment copies it)." }
  - { name: pod_name, type: varchar, description: "Pod that ran this try, from the pod's bound token on first contact; cross-checked by the watch." }
  - { name: pod_uid, type: varchar, description: "Pod UID, recorded from the token's claim; the watch confirms it." }
  - { name: node_name, type: varchar, description: "Node the pod landed on." }
  - { name: image_digest, type: varchar, description: "Image digest this try ran, so a deploy mid-run is legible." }
  - { name: snapshot_start, type: bigint, description: "Lake snapshot before the task's first commit." }
  - { name: snapshot_end, type: bigint, description: "Lake snapshot of the task's last commit: the durable reference downstream pins to." }
  - { name: counters, type: json, description: "Progress counters, replaced on every heartbeat and at finish." }
  - { name: trace_context, type: varchar, description: "traceparent of this try's span, a child of jobs.trace_context." }
  - { name: token_hash, type: varchar, description: "The fallback credential only (§6.4): SHA-256 of the per-attempt token, compared with ct_eq; NULL under the projected-token path; nulled at finish." }
  - { name: scheduled_at, type: timestamp, volatile: true, description: "Readiness fired." }
  - { name: queued_at, type: timestamp, volatile: true, description: "Launch claimed (write-ahead, before the create)." }
  - { name: started_at, type: timestamp, volatile: true, description: "Pod reported start." }
  - { name: heartbeat_at, type: timestamp, volatile: true, description: "Last heartbeat." }
  - { name: finished_at, type: timestamp, volatile: true, description: "Terminal state recorded." }
  - { name: created_at, type: timestamp, nullable: false, default: "now()", volatile: true, description: "Row created." }
  - { name: updated_at, type: timestamp, nullable: false, default: "now()", volatile: true, description: "Row updated." }
indexes:
  - { name: task_instances_org_job, columns: [org_id, job_id] }                       # non-partial, leads with org_id: V15 holds, tree_is_clean's debt list is unchanged
  - { name: task_instances_live, columns: [job_id, task_id], unique: true, where: "live IS TRUE" }
  - { name: task_instances_one_pod_per_org, columns: [org_id], unique: true, where: "live IS TRUE AND kind = 'pod' AND state IN ('queued','running','restarting','canceling')" }
  - { name: task_instances_claimable, columns: [state, not_before, job_id, seq], where: "live IS TRUE" }
  - { name: task_instances_heartbeat, columns: [heartbeat_at], where: "live IS TRUE AND state = 'running'" }
```

`natural_key` renders `UNIQUE (job_id, task_id, try_number)` — Airflow's `task_instance_try_key`; `task_instances_live` is its `task_instance_current_key`. Registering `TaskInstance` in `ApiDoc` ([openapi.rs](../rust/crates/er-server/src/openapi.rs)) bumps `every_generated_entity_schema_is_registered` from 12 to 13.

### 4.3 The lake ledger: `entities/run_stage/entity.yaml`

`run_stages` exists only as an `er-schema` DDL spec with no writer ([er-schema/src/model.rs](../rust/crates/er-schema/src/model.rs) `ddl_specs`, marked RESERVED). It becomes a **full** lake entity — `stores: [duckdb]`, `key: [id]`, `natural_key: [run_id, stage, try_number]` — because the lint registry admits nothing less (V1–V4: a NOT NULL `id` first, the key exactly `[id]`, NOT NULL `created_at`/`updated_at`): `id` varchar NOT NULL first, `er_ids::natural_id('run_stages', [run_id, stage, try_number])` with the same `sha256('run_stages:' || …)` SQL mirror the task table uses; then the spec's existing columns `run_id, stage, seq, status, started_at, ended_at, snapshot_start, snapshot_end`, the promoted counters, `counters, error_class, error_detail`; then `try_number` bigint NOT NULL (no `default` — V11 forbids defaults on a DuckLake entity; the writer supplies 1) and `task_instance_id` varchar nullable (no `references`: V11, and it is cross-store); then `created_at`/`updated_at` timestamp NOT NULL, not volatile (V4 + V6), `created_at` the stage's start stamp on the run's logical clock and `updated_at` advanced by `finish.sql` (E014), mirroring [entities/run/entity.yaml](../entities/run/entity.yaml). No `accepted_values` on `stage`/`status` (V11); their domains stay in the hand-written tspec's `enum_map`, as for `runs`. The hand-written `run_stages` tspec in `er_schema::model::ddl_specs()` takes the same columns in the same order and nullability, and its keys `(run_id, seq)`/`(run_id, stage)` — which cannot stay unique with one row per try — become `key(["id"])` + `key(["run_id","stage","try_number"])`, since `to_lake_table_spec` publishes `[id]` plus the natural key and `every_entity_lake_table_matches_the_s5_registry` (er-entity-gen `tests/registry_matches_entities.rs`) byte-compares `create_table_sql` on both sides. The same change adds `finish` and `apply_staged` to `RUN_STAGES`, moves the `assert_eq!(RUN_STAGES.len(), 9)` pin to 11, regenerates `tests/model_registry_golden.json` (columns, keys and the `stage` enum lists) and lets `er-entity gen` regenerate erd.md; `declared_names.rs` needs nothing, since it resolves through `all_specs()`, which already lists `run_stages`.

There is no additive constraint on the shape: no lake this engine has created holds `run_stages` — `create_ddl_table` is never called for it and the reconciler creates only entity-catalog relations — so the reconciler creates the new table (Safe: "relation absent; create table") in every tenant lake at the next boot or lake job. A foreign table of that name lacking `id` or the timestamps is correctly refused with `ERR_SCHEMA_BREAKING`; the operator remedy is to drop or rename it, and the relation keeps its name because `run_stages` is already the name er-errors, er-obs, erd.md, [frontend-design.md](frontend-design.md) and [infrastructure.md](infrastructure.md) use.

The pod writes `run_stage/create/start.sql` before its first lake commit and `run_stage/update/finish.sql` as its **last dedicated commit** after the stage's data commits. That ordering is what makes the ledger a durable reference: a `succeeded` row means the data commits happened. `task_instance_id`, not `try_number`, is the ledger's join key for the probe (§6.3 step 4): the probe matches any `succeeded` row whose `task_instance_id` is in the current try's retry lineage, so a zombie that committed under try *n* is found by the try *n+1* pod; `try_number` stays for display. `entities/run/update/fail.sql` is declared so `finish` can write `runs.status` to `failed` or `canceled` — `RUN_STATUSES` has declared `failed` with no writer, and today a run that dies mid-chain is `running` forever, hidden from the drift guard only by `latest_fingerprint.sql`'s `status = 'succeeded'` filter; `canceled` joins `RUN_STATUSES` (the enum map, the golden and the pin move, as for `RUN_STAGES`). `finish` consumes no lake edge (`Input = ()`), so its work order carries the leaf verdicts the readiness pass composed.

The entity, abbreviated (the eleven promoted counters from `er_schema::model::PROMOTED_COUNTERS` sit between `snapshot_end` and `counters`, one bigint each):

```yaml
kind: entity
version: 1
name: run_stage
relation: run_stages
description: "The lake-side stage ledger (S5.2): one row per try of one stage of one run; the durable reference a pod probes before it executes."
key: [id]
natural_key: [run_id, stage, try_number]
generate: { stores: [duckdb], languages: [rust] }
columns:
  - { name: id, type: varchar, nullable: false, description: "er_ids::natural_id('run_stages', [run_id, stage, try_number])." }
  - { name: run_id, type: varchar, nullable: false, description: "The run (jobs.id)." }
  - { name: stage, type: varchar, nullable: false, description: "A RUN_STAGES member; the domain stays in the registry's enum_map (V11)." }
  - { name: seq, type: bigint, nullable: false, description: "Position in the run." }
  - { name: status, type: varchar, nullable: false, description: "running | succeeded | failed (RUN_STATUSES)." }
  - { name: started_at, type: timestamp, nullable: false, description: "Run-logical start stamp." }
  - { name: ended_at, type: timestamp, description: "Run-logical end stamp; NULL while running." }
  - { name: snapshot_start, type: bigint, description: "Lake snapshot before the stage's first commit." }
  - { name: snapshot_end, type: bigint, description: "Lake snapshot of the stage's last data commit." }
  - { name: counters, type: json, description: "The full counter set." }
  - { name: error_class, type: varchar, description: "er-errors class on failure." }
  - { name: error_detail, type: varchar, description: "Chain-flattened text." }
  - { name: try_number, type: bigint, nullable: false, description: "The task try that wrote the row (display); the writer supplies 1 outside the DAG path." }
  - { name: task_instance_id, type: varchar, description: "The task_instances.id that wrote the row: the probe's join key. No references — cross-store." }
  - { name: created_at, type: timestamp, nullable: false, description: "The stage's start on the run's logical clock." }
  - { name: updated_at, type: timestamp, nullable: false, description: "Advanced by finish.sql." }
```

Its statements follow [entities/run/update/finish.sql](../entities/run/update/finish.sql)'s form — `mode: partial`, `returns: none`, timestamps as text parameters cast in the SQL:

```sql
-- run_stage/create/start.sql
INSERT INTO lake.main.run_stages
  (id, run_id, stage, seq, status, started_at, snapshot_start, try_number, task_instance_id, created_at, updated_at)
VALUES ($id, $run_id, $stage, $seq, 'running', $started_at::timestamp, $snapshot_start, $try_number, $task_instance_id,
        $started_at::timestamp, $started_at::timestamp)

-- run_stage/update/finish.sql
UPDATE lake.main.run_stages
SET status = $status, ended_at = $ended_at::timestamp, snapshot_end = $snapshot_end, counters = $counters::json,
    error_class = $error_class, error_detail = $error_detail, updated_at = $ended_at::timestamp
WHERE id = $id

-- run/update/fail.sql ($status: failed | canceled)
UPDATE lake.main.runs SET status = $status, ended_at = $ended_at::timestamp, updated_at = $ended_at::timestamp
WHERE id = $run_id

-- run_stage/read/succeeded_in_lineage.sql  (pin: live; pin_reason: the writer reading its own ledger before it writes)
SELECT id, task_instance_id, snapshot_start, snapshot_end, counters
FROM lake.main.run_stages
WHERE run_id = $run_id AND stage = $stage AND status = 'succeeded' AND list_contains($task_instance_ids, task_instance_id)
```

### 4.4 Idempotency statements

Each is a declared statement through `entities/` and the S5 registry: `run/delete/by_id.sql` (so `init` can delete-then-insert the run row — DuckLake enforces no keys, and `run` has no `natural_key` for a MERGE), `raw_record/delete/by_batch.sql` + `ingest_batch/delete/by_run.sql` (so `ingest` pre-images the whole run and re-lands every delivery: the batch id *is* the run id — `run_all_ensemble` passes `inputs.run_id` as `run_ingest`'s `ingest_batch_id`, and `raw_records` carries no per-source key beyond `source_system` — so a per-source skip has nothing to key on, §9.1), `entity_event/delete/by_run.sql` (so `reconcile` deletes its own run's events before appending — which also fixes the doubled webhook aggregate). `train` writes under `models/<run_id>/` and the registry row carries the TF path; see §9.1. The ledger probe's two reads are declared beside them: `task_instance/read/retry_lineage.sql` (Postgres: the current id plus the archived `retry` predecessors of `(job_id, task_id)`, walked back contiguously to the first `cleared`/`restarted` row or try 1) and `run_stage/read/succeeded_in_lineage.sql` (lake, `pin: live` with its `pin_reason` — the writer reading its own ledger: `status = 'succeeded' AND run_id = $run_id AND stage = $stage AND list_contains($task_instance_ids, task_instance_id)`).

### 4.5 The critical statements

Every statement is one declared `.sql` + `.yaml` pair. Where a decision archives one row and inserts its successor, it is two statements run in one transaction on the per-request connection (`client.transaction()`, the §4 preface); the prose names the pair by its decision (`fail_or_retry`, `clear`, `complete_restart`) and the files by name. `RETURNING <all columns>` abbreviates the full list E003 requires. Every guard names the row's state **and** the identity of the launch or pod reporting (`k8s_job_name`, `pod_uid`, `owner_epoch`), so a duplicate watch event for an old launch, a stale pod, or a previous dispatcher epoch matches zero rows — zero rows is the no-op, never an error, exactly as `pg.rs` treats a lost `claim`.

**Planning is one transaction.** `claim.sql` (unchanged), one `task_instance/create/plan.sql` per node and `job/update/plan.sql` (`dispatching → running`, writes `run_id = id`, both hashes, `image_digest`, `owner_epoch`) run in **one** Postgres transaction on the dispatcher's connection. A planning refusal — a `jobs.kind` that does not parse, an assembly failure — writes the terminal `config`/`internal` verdict in the same transaction through `job/update/refuse_plan.sql` (`dispatching → failed`, `error_class`, `error_detail`, `finished_at`): the one job statement besides `plan.sql` that leaves `dispatching`. A crash or a dropped connection mid-planning rolls back to `queued` with `attempt` unchanged and zero task rows, and the job is simply re-claimed; `dispatching` is therefore never a committed state on the DAG path. It stays in the `jobs_one_active_per_org` predicate and in `cancel.sql` for the legacy arm, because an index change on a live table is not an additive reconcile.

**`task_instance/create/plan.sql`** — one row per node; idempotent, so re-planning after a clear inserts nothing for rows that exist:

```sql
INSERT INTO public.task_instances
  (id, org_id, job_id, task_id, seq, try_number, live, kind, state, trigger_rule, upstream, teardown,
   segment, resource_class, max_tries, max_launches, image_digest, trace_context)
VALUES ($id, $org_id, $job_id, $task_id, $seq, 1, TRUE, $kind, 'none', $trigger_rule, $upstream, $teardown,
   $segment, $resource_class, $max_tries, $max_launches, $image_digest, $trace_context)
ON CONFLICT (job_id, task_id, try_number) DO NOTHING
RETURNING <all columns>
```

`input` is not written at planning: the readiness pass composes the whole work order (the `RunPlan` from `init`'s `output`, the job's params, the pins from each upstream's `output`/`snapshot_end`) at every verdict, so a pin exists only on `scheduled | up_for_retry | queued | running` rows and was written by `mark_scheduled`, copied by `insert_retry_try` from a row that passed it, or written by `advance_segment` for a fused member.

**`task_instance/update/mark_scheduled.sql`** — the readiness evaluator's verdict, guarded. Every transaction that treats a job's rows as a set — the readiness pass, the clear, the finish-and-advance of a fused segment — opens with a declared **`job/read/lock.sql`** (`SELECT id, state, clear_number FROM public.jobs WHERE id = $job_id FOR UPDATE`), so the three serialize on the job row: a `clear_number` predicate alone would not do it, because under READ COMMITTED a correlated sub-select answers from the statement's snapshot and does not see a clear that committed while the UPDATE waited on a row lock. The `clear_number` predicate stays on `mark_scheduled` and `propagate` as belt and braces, and a zero-row verdict rolls the whole pass back rather than continuing. Readiness itself is evaluated in Rust from the compiled graph (§5.3); SQL only records it, and always writes the pin whole:

```sql
UPDATE public.task_instances
SET state = 'scheduled', scheduled_at = now(), input = $input, updated_at = now()
WHERE id = $task_instance_id AND live IS TRUE AND state = 'none'
  AND (SELECT COALESCE(j.clear_number, 0) FROM public.jobs j WHERE j.id = task_instances.job_id) = $clear_number
RETURNING <all columns>
```

**`task_instance/update/claim_launch.sql`** — the launcher's SKIP LOCKED hand-out. It counts the launch, writes the Job name and the owner epoch **before** any Kubernetes call (dagr's write-ahead `attempt-submitted` record, `crates/cli/src/k8s_runner.rs:665-673`), honours the in-flight cap, and locks only the task row (`FOR UPDATE OF t`, as `claim.sql` locks only `jobs`), so a concurrent `heartbeat` or job `settle` holding the job row neither blocks nor skips it:

```sql
UPDATE public.task_instances
SET state = 'queued', queued_at = now(), owner_epoch = $owner_epoch,
    launches = task_instances.launches + 1, k8s_job_name = $k8s_job_name, updated_at = now()
WHERE id = (
  SELECT t.id FROM public.task_instances t
  JOIN public.jobs j ON j.id = t.job_id
  WHERE t.live IS TRUE AND t.kind = 'pod'
    AND t.state IN ('scheduled', 'up_for_retry')
    AND (t.not_before IS NULL OR t.not_before <= now())
    AND (j.state = 'running' OR (j.state = 'canceling' AND t.teardown))
    AND NOT EXISTS (SELECT 1 FROM public.task_instances a                       -- the one-slot-per-tenant pool
                    WHERE a.org_id = t.org_id AND a.live IS TRUE AND a.kind = 'pod'
                      AND a.state IN ('queued', 'running', 'restarting', 'canceling'))
    AND $in_flight_cap > (SELECT count(*) FROM public.task_instances c           -- the only capacity number we hold
                          WHERE c.live IS TRUE AND c.kind = 'pod' AND c.state IN ('queued', 'running'))
  ORDER BY j.priority DESC, j.id, t.seq
  FOR UPDATE OF t SKIP LOCKED
  LIMIT 1)
RETURNING <all columns>
```

A `UNIQUE_VIOLATION` on `task_instances_one_pod_per_org` maps to `Ok(None)` exactly as `pg.rs` maps the jobs index today. Before the Kubernetes create the launcher re-reads the row and requires `state = 'queued' AND try_number = $try AND owner_epoch = $mine` — Airflow's stale-workload check (`kubernetes_executor.py:479-546`); any mismatch drops the create.

**`task_instance/update/claim_control.sql`** / **`finish_control.sql`** — the control-task halves of the same loop (§3.6). The claim is `claim_launch` without the org-slot and cap predicates and with `kind = 'control'`, `state = 'running'`, `started_at = now()`; the finish has `finish.sql`'s SET list fenced by `kind = 'control' AND owner_epoch = $owner_epoch` — the dispatcher epoch is the control fence, as `pod_uid` is the pod fence, so `finish.sql`/`heartbeat.sql` can never match a control row.

**`task_instance/update/start.sql`** — the pod's first report. Zero rows means the pod is stale (a different launch, a finished attempt, a canceled one) and it exits 3 after logging why:

```sql
UPDATE public.task_instances
SET state = 'running', pod_name = $pod_name, pod_uid = $pod_uid, node_name = $node_name,
    image_digest = $image_digest, started_at = now(), heartbeat_at = now(), updated_at = now()
WHERE id = $task_instance_id AND live IS TRUE AND state = 'queued' AND k8s_job_name = $k8s_job_name
RETURNING <all columns>
```

**`task_instance/update/advance_segment.sql`** — the fused pod's hand-off from member *k* to *k+1* (§3.7), run by the `finish` handler in the same transaction, opened by `job/read/lock.sql`, immediately after `finish.sql` succeeded for *k*: finish first, advance second, two statements, so the one-pod index sees *k* leave its predicate before *k+1* enters it. The handler passes *k*'s RETURNING values as parameters, checks from the compiled graph that `$next_task_instance_id` is *k*'s immediate successor inside the same segment with every upstream success-like, and composes `$input` exactly as the readiness pass would:

```sql
UPDATE public.task_instances
SET state = 'running', k8s_job_name = $k8s_job_name, owner_epoch = $owner_epoch,
    pod_name = $pod_name, pod_uid = $pod_uid, node_name = $node_name, image_digest = $image_digest,
    input = $input, launches = 1,
    scheduled_at = now(), queued_at = now(), started_at = now(), heartbeat_at = now(), updated_at = now()
WHERE id = $next_task_instance_id AND live IS TRUE AND kind = 'pod' AND state = 'none'
  AND job_id = $job_id AND segment = $segment AND resource_class = $resource_class
  AND (SELECT COALESCE(j.clear_number, 0) FROM public.jobs j WHERE j.id = task_instances.job_id) = $clear_number
RETURNING <all columns>
```

Zero rows (`cancel_pending` moved *k+1* to `canceled`, or a clear touched *k+1* — a class override or a rotation — since the pod's work order was issued) is reported to the pod as "no advance": it exits 0 with *k*'s record, and the remaining members go through readiness → `claim_launch` as a new segment headed at *k+1*. The `finish` request carries `next: Option<task_instance_id>` — `None` for the last member and on failure — so the route set stays at four.

**`task_instance/update/heartbeat.sql`** — every 30 s from a side thread (the DuckDB thread is blocked). Zero rows sets the pod's **fence flag**: the task checks it before its last write-back and rolls back instead of committing:

```sql
UPDATE public.task_instances
SET heartbeat_at = now(), counters = COALESCE($counters, counters), updated_at = now()
WHERE id = $task_instance_id AND live IS TRUE AND state = 'running' AND pod_uid = $pod_uid
RETURNING <all columns>
```

**`task_instance/update/finish.sql`** — the pod's terminal success report, after its last lake commit, guarded so a late report never overwrites a cancel, a clear or a timeout:

```sql
UPDATE public.task_instances
SET state = CASE WHEN $skipped THEN 'skipped' ELSE 'succeeded' END,
    outcome = $outcome, output = $output, snapshot_start = $snapshot_start, snapshot_end = $snapshot_end,
    counters = COALESCE($counters, counters), exit_code = $exit_code, finished_at = now(), updated_at = now()
WHERE id = $task_instance_id AND live IS TRUE AND state = 'running' AND pod_uid = $pod_uid
RETURNING <all columns>
```

**`task_instance/update/record_evidence.sql`** — what the watch writes on a row that already has its verdict: the observed `exit_code` and `termination_reason` of a pod that reached a terminal phase after `finish` landed. Evidence, never a verdict (§6.5). For a fused pod it is written on at most one member — the last that reported `finish` in that pod — never on the earlier members that share its `pod_uid`:

```sql
UPDATE public.task_instances
SET exit_code = $exit_code, termination_reason = $termination_reason, updated_at = now()
WHERE id = $task_instance_id AND pod_uid = $pod_uid AND k8s_job_name = $k8s_job_name AND state IN ('succeeded', 'skipped')
RETURNING <all columns>
```

**`task_instance/update/note_interruption.sql`** — the pod's `interrupted` record (SIGTERM, fence, §5.6): the counters and the reason, **no state change**. The row stays `running`, `restarting` or `canceling` until the watch rules on the platform's evidence:

```sql
UPDATE public.task_instances
SET termination_reason = $termination_reason, counters = COALESCE($counters, counters), updated_at = now()
WHERE id = $task_instance_id AND live IS TRUE AND state IN ('running', 'restarting', 'canceling') AND pod_uid = $pod_uid
RETURNING <all columns>
```

**`fail_or_retry`** = **`task_instance/update/archive_try.sql`** + **`task_instance/create/insert_retry_try.sql`**, in one transaction — the scheduler's decision once the pod's failure report or the watch's terminal classification arrives (whichever first; the guards make the second a no-op). Airflow's `handle_failure` + `prepare_db_for_next_try` (`taskinstance.py:1736-1764`, `:2802-2816`): archive the live row **forced to `failed`** whether or not a successor is inserted (Airflow archives a non-finished row as FAILED, `:1645-1677`, so history reads correctly), then insert the next try with Airflow's budget arithmetic — eligible while `try_number <= max_tries` — deterministic jitter in `[min, 2·min)`, base and cap per kind (30 s / 900 s for a pod, 5 s / 60 s for a control task), and the OOM escalation. The framework has no multi-statement form and E012 (verb matches folder) is an AST rule that runs under `mode: partial` too, so a `WITH … UPDATE … INSERT` is refused in any write folder; two files are the design, not a fallback. The guard requires the job to be `running` (or `canceling`, for the teardown row alone): a failure report that lands after a cancel is a no-op, and the cancel sweep settles the row (§5.6). `restarting` is excluded: a cleared live row is rotated by the watch (`complete_restart`), never failed by its pod.

```sql
-- update/archive_try.sql
UPDATE public.task_instances
SET state = $terminal_state,                                    -- failed | timed_out
    live = CASE WHEN $retryable AND try_number <= max_tries THEN NULL ELSE TRUE END,
    archived_reason = CASE WHEN $retryable AND try_number <= max_tries THEN 'retry' ELSE archived_reason END,
    exit_code = $exit_code, termination_reason = $termination_reason,
    error_class = $error_class, error_detail = $error_detail, finished_at = now(), updated_at = now()
WHERE id = $task_instance_id AND live IS TRUE AND state IN ('queued', 'running')
  AND k8s_job_name IS NOT DISTINCT FROM $k8s_job_name                          -- NULL = NULL for a control row
  AND EXISTS (SELECT 1 FROM public.jobs j WHERE j.id = task_instances.job_id
              AND (j.state = 'running' OR (j.state = 'canceling' AND task_instances.teardown)))
RETURNING <all columns>
```

```sql
-- create/insert_retry_try.sql, run only when the archived row came back with live IS NULL
INSERT INTO public.task_instances
  (id, org_id, job_id, task_id, seq, try_number, live, kind, state, trigger_rule, upstream, teardown, segment,
   resource_class, escalated_from, max_tries, max_launches, not_before, input, image_digest, trace_context)
SELECT encode(sha256(convert_to('task_instances:' || job_id || ':' || task_id || ':' || (try_number + 1)::text, 'UTF8')), 'hex'),
       org_id, job_id, task_id, seq, try_number + 1, TRUE, kind, 'up_for_retry', trigger_rule, upstream, teardown,
       CASE WHEN $escalate THEN NULL ELSE segment END,                 -- a class change invalidates the segment; the launcher re-derives it (§3.7)
       CASE WHEN $escalate AND resource_class = 'S' THEN 'M'
            WHEN $escalate AND resource_class = 'M' THEN 'L' ELSE resource_class END,
       CASE WHEN $escalate THEN resource_class ELSE NULL END,
       max_tries, max_launches,
       now() + make_interval(secs =>
         least($backoff_cap, $backoff_base * power(2, try_number - 1))::int
         + mod(abs(hashtext(job_id || '#' || task_id || '#' || try_number::text)::bigint),      -- ::bigint first: abs(int4 min) overflows
               least($backoff_cap, $backoff_base * power(2, try_number - 1))::int)),
       input, image_digest, trace_context
FROM public.task_instances
WHERE id = $task_instance_id AND live IS NULL AND archived_reason = 'retry'
  AND NOT EXISTS (SELECT 1 FROM public.task_instances l
                  WHERE l.job_id = task_instances.job_id AND l.task_id = task_instances.task_id AND l.live IS TRUE)
RETURNING <all columns>
```

The successor id is the SQL mirror of `er_ids::natural_id`, which hashes `relation` then `':' || part` for each part ([er-ids/src/lib.rs](../rust/crates/er-ids/src/lib.rs) `natural_id`); a test asserts byte-equality the way `natural_id_matches_the_sql_recipe` already does for DuckDB. Precedent for an `INSERT … SELECT` under `create/`: [entities/config_version/create/draft.sql](../entities/config_version/create/draft.sql).

**`task_instance/update/requeue_launch.sql`** — a pod that never ran user code (a fatal waiting reason, a Job create the API refused for a non-capacity reason, a Job that never got a pod, a pod that vanished while `queued`, a pod that could not reach the control plane) or was disrupted while running — written only once the pod object is terminal or gone (§6.5, clause d): back to `scheduled` within the infrastructure budget, no try spent. Zero rows means a stale launch (no-op) **or** an exhausted budget; the handler re-reads the row and, when `launches = max_launches`, runs `fail_or_retry` with `error_class = infrastructure`:

```sql
UPDATE public.task_instances
SET state = 'scheduled', k8s_job_name = NULL, pod_name = NULL, pod_uid = NULL, node_name = NULL,
    not_before = $not_before, termination_reason = $termination_reason, updated_at = now()
WHERE id = $task_instance_id AND live IS TRUE AND state IN ('queued', 'running')
  AND k8s_job_name = $k8s_job_name AND launches < max_launches
RETURNING <all columns>
```

**`task_instance/update/capacity_wait.sql`** — capacity is never a tenant's failure and spends no budget: a Job create rejected `403 … exceeded quota` (object-count quota), a `FailedCreate` whose message says `exceeded quota` (compute quota, charged to the pod the Job controller tries to create), and `PodScheduled=False/Unschedulable` past the 10-minute bound (§5.7) all delete the Job and return the row to `scheduled` with `launches = launches - 1`, `not_before = now() + 30 s` and the reason in `termination_reason`; `er_quota_waits_total{reason}` increments — the 2.3 incident's "bounded, logged quota-wait" ([plan-to-aws.md](plan-to-aws.md) 2.3). The delete comes first and the wait is recorded only when the watch reports the Job gone: `launches - 1` re-derives the same Job name on the next claim, so a recreate before the delete completed would `409` into the launch budget. The Job is never left in place to let the controller's own backoff be the wait, because `activeDeadlineSeconds` runs from `status.startTime` and a long quota wait would surface as `DeadlineExceeded → timed_out`, charging a try to a task that never ran. Charging a namespace ceiling to a tenant's budget is the failure dagr ADR 115 §6 exists to prevent.

```sql
UPDATE public.task_instances
SET state = 'scheduled', launches = launches - 1, k8s_job_name = NULL, pod_name = NULL, pod_uid = NULL,
    not_before = now() + make_interval(secs => $wait_secs), termination_reason = $termination_reason, updated_at = now()
WHERE id = $task_instance_id AND live IS TRUE AND state = 'queued' AND k8s_job_name = $k8s_job_name AND launches > 0
RETURNING <all columns>
```

**`clear`** = **`restart_cleared.sql`** + **`archive_cleared.sql`** + **`reset_pending.sql`** + **`task_instance/create/insert_cleared_try.sql`** (one per archived row) + **`job/update/reopen.sql`**, in one transaction. "Retry this task": the handler computes the closure from the compiled graph (§5.4) and passes it as `$task_ids`; `$retries` is `NodePolicy::retries` from `dag_for(jobs.kind)`, as Airflow reads `task.retries` from the DAG. Three arms, by the row's state:

- **live `queued | running`** → `restarting`, recording the optional class override for the successor (`next_resource_class = $resource_class`); the Job is revoked and deleted, and the watch rotates the row when the pod is gone (`complete_restart`, below; Airflow `scheduler_job_runner.py:1664-1672`).
- **terminal** → archived with `archived_reason = 'cleared'`, successor in `none` with `max_tries = try_number + $retries` (Airflow `taskinstance.py:548`, cumulative).
- **pending `none | scheduled | up_for_retry`** → reset **in place** to `none` with no pin and a re-based budget: no try was consumed, so no row rotates (Airflow's pending-attempt rule, `taskinstance.py:526-531`). This is what stops a `scheduled` downstream, pinned to the upstream snapshot the operator just discarded, from being launched first and computing from data that no longer counts.

```sql
-- update/restart_cleared.sql
UPDATE public.task_instances
SET state = 'restarting', next_resource_class = $resource_class, updated_at = now()
WHERE org_id = $org_id AND job_id = $job_id AND live IS TRUE AND state IN ('queued', 'running')
  AND task_id IN (SELECT value FROM jsonb_array_elements_text($task_ids))
RETURNING <all columns>
```

```sql
-- update/archive_cleared.sql
UPDATE public.task_instances
SET live = NULL, archived_reason = 'cleared', updated_at = now()
WHERE org_id = $org_id AND job_id = $job_id AND live IS TRUE
  AND state IN ('succeeded', 'skipped', 'failed', 'timed_out', 'upstream_failed', 'upstream_skipped', 'canceled')
  AND task_id IN (SELECT value FROM jsonb_array_elements_text($task_ids))
RETURNING <all columns>
```

```sql
-- update/reset_pending.sql
UPDATE public.task_instances
SET state = 'none', input = NULL, not_before = NULL, scheduled_at = NULL,
    max_tries = try_number - 1 + $retries,                       -- Airflow: previous_try_number + retries
    resource_class = COALESCE($resource_class, resource_class), image_digest = $image_digest, updated_at = now()
WHERE org_id = $org_id AND job_id = $job_id AND live IS TRUE AND state IN ('none', 'scheduled', 'up_for_retry')
  AND task_id IN (SELECT value FROM jsonb_array_elements_text($task_ids))
RETURNING <all columns>
```

```sql
-- create/insert_cleared_try.sql: the successor of a `cleared` or `restarted` row; input NULL (mark_scheduled pins it)
INSERT INTO public.task_instances
  (id, org_id, job_id, task_id, seq, try_number, live, kind, state, trigger_rule, upstream, teardown, segment,
   resource_class, max_tries, max_launches, image_digest, trace_context)
SELECT encode(sha256(convert_to('task_instances:' || job_id || ':' || task_id || ':' || (try_number + 1)::text, 'UTF8')), 'hex'),
       org_id, job_id, task_id, seq, try_number + 1, TRUE, kind, 'none', trigger_rule, upstream, teardown, segment,
       COALESCE($resource_class, next_resource_class, resource_class), try_number + $retries, max_launches,
       $image_digest, trace_context
FROM public.task_instances
WHERE id = $task_instance_id AND live IS NULL AND archived_reason IN ('cleared', 'restarted')
  AND NOT EXISTS (SELECT 1 FROM public.task_instances l
                  WHERE l.job_id = task_instances.job_id AND l.task_id = task_instances.task_id AND l.live IS TRUE)
RETURNING <all columns>
```

Then **`job/update/reopen.sql`**: `SET clear_number = COALESCE(clear_number, 0) + 1, state = CASE WHEN state IN ('succeeded','failed','canceled') THEN 'queued' ELSE state END, finished_at = NULL, error_class = NULL, error_detail = NULL, not_before = now(), updated_at = now() WHERE id = $job_id AND org_id = $org_id`. A finished job goes back to `queued`, so the existing `claim.sql` re-acquires the org slot through the existing index and the existing priority order; planning then inserts nothing (ON CONFLICT), `clear_number` fences any readiness verdict from before the clear, and the next pass finds the cleared rows in `none` with their upstream already `succeeded`. The handler refuses (409) a clear under a `canceling` job and recomputes segments after the rotation (§3.7).

**`complete_restart`** = **`task_instance/update/archive_restarted.sql`** + **`insert_cleared_try.sql`**, in one transaction, run by the watch when the pod of a `restarting` row is terminal — Airflow's `complete_restart`: the operator's clear wins whatever the old pod reported (an OOMKill, a genuine exit 1, a SIGTERM), so no classification, no escalation, no try charged; the evidence is kept and `error_class` stays NULL, because a restart is evidence, not an error:

```sql
-- update/archive_restarted.sql
UPDATE public.task_instances
SET state = 'failed', live = NULL, archived_reason = 'restarted',
    exit_code = $exit_code, termination_reason = $termination_reason, finished_at = now(), updated_at = now()
WHERE id = $task_instance_id AND live IS TRUE AND state = 'restarting'
  AND k8s_job_name IS NOT DISTINCT FROM $k8s_job_name
RETURNING <all columns>
```

**Cancel, adoption, propagation.** `job/update/cancel.sql`: `SET state = CASE WHEN state = 'queued' THEN 'canceled' ELSE 'canceling' END WHERE id = $job_id AND org_id = $org_id AND state IN ('queued', 'dispatching', 'running')`. `task_instance/update/cancel_pending.sql`: every `none | scheduled | up_for_retry` row of the job that is not `teardown` → `canceled` (the teardown `finish` stays, becomes ready once every upstream is terminal, and is claimable under `canceling`). `task_instance/update/cancel_live.sql`: every live `queued | running | restarting` row of either kind **except a `teardown` row** → `canceling`, the write-ahead marker before a pod row's Job is revoked and deleted; for a control row the evidence that ends the wait is the body's return, so the sweep runs `settle_canceled` on a `canceling` control row that is not in the dispatcher's own in-flight set. `task_instance/update/settle_canceled.sql`: `SET state = 'canceled', exit_code = $exit_code, termination_reason = $termination_reason, finished_at = now(), updated_at = now() WHERE id = $task_instance_id AND live IS TRUE AND state = 'canceling' AND k8s_job_name IS NOT DISTINCT FROM $k8s_job_name` — when the watch reports the pod gone. `task_instance/update/propagate.sql`: `SET state = $state WHERE id = $id AND live IS TRUE AND state = 'none' AND (SELECT COALESCE(j.clear_number, 0) FROM public.jobs j WHERE j.id = task_instances.job_id) = $clear_number` for `upstream_failed | upstream_skipped | canceled`. `task_instance/update/adopt.sql`: `SET owner_epoch = $epoch WHERE live IS TRUE AND state IN ('queued','running','restarting','canceling') RETURNING …`, and `job/update/adopt.sql`, the same plain shape over `jobs` (`WHERE state IN ('running','canceling')`) — the one reader of `jobs.owner_epoch` — so the start-up pass enumerates jobs before their rows (§6.6). `job/update/settle.sql` carries the guard `mark.sql` lacks today, `WHERE id = $job_id AND state IN ('running', 'canceling')`. `mark.sql` survives only for the legacy arm and is deleted with it.

### 4.6 One writer per tenant

Two partial unique indexes, zero code: `jobs_one_active_per_org` (unchanged — a run holds the org slot from claim through `canceling`) and `task_instances_one_pod_per_org` (at most one live pod per tenant — `queued | running | restarting | canceling` — which a two-branch DAG, a cleared task racing a teardown, or a fused segment's hand-off could otherwise violate; `advance_segment` runs after `finish` in its transaction for exactly this reason). Assembly refuses a graph with two unordered lake-writing tasks (§3.3), so the index should never fire; it is there because the queue's correctness is Postgres's job, not code's. A pod-name uniqueness index is redundant: the Job name is a pure function of the attempt key and launch count, and the Job controller gives the pod the Job's name as prefix.

## 5. The state machine

### 5.1 States and their writers

| State | Meaning | Written by | Statement |
|---|---|---|---|
| `none` | planned, upstream not yet terminal; also a pending row reset in place by a clear, or the successor of a rotated row | dispatcher at planning; operator (clear); the watch (`complete_restart`) | `plan`, `reset_pending`, `insert_cleared_try` |
| `scheduled` | trigger rule fired; waiting for the org slot, the in-flight cap, or a capacity wait | dispatcher readiness pass; the watch | `mark_scheduled`, `requeue_launch`, `capacity_wait` |
| `queued` | launch claimed (write-ahead); Job created or being created | dispatcher launcher | `claim_launch` |
| `running` | pod reported start; a fused member advanced by its pod; a control body running | pod via channel; `finish` handler; dispatcher | `start`, `advance_segment`, `claim_control` |
| `up_for_retry` | next try row waiting for `not_before` | dispatcher | `insert_retry_try` |
| `restarting` | cleared while live; Job being deleted; rotated when the pod is gone | operator (clear) | `restart_cleared` |
| `canceling` | job canceled while live; Job being deleted; settled when the pod is gone | dispatcher cancel sweep | `cancel_live` |
| `succeeded` / `skipped` | terminal, success-like / skip-like | pod via channel, confirmed by the watch; dispatcher for a control row | `finish`, `finish_control` |
| `failed` / `timed_out` | terminal, failure-like | dispatcher, from the pod's failure report or the watch's evidence | `archive_try` (+ `insert_retry_try`) |
| `upstream_failed` / `upstream_skipped` | never ran: rule can never fire | dispatcher readiness pass | `propagate` |
| `canceled` | stop-like | dispatcher: pending rows at cancel; `canceling` rows when the pod is gone | `cancel_pending`, `settle_canceled`, `propagate` |

Control tasks use the same states without `queued`: `scheduled → running` (`claim_control`, committed) → the body → terminal (`finish_control` or `fail_or_retry`), three commits (§3.6).

**The loop.** Today's loop shape stays ([dispatcher.rs](../rust/crates/er-server/src/dispatcher.rs) `run_loop`: idle backoff 500 ms → 5 s, the 20 s schedule tick, the webhook drain). Between the tick and the drain it runs, in order: the cancel sweep; the readiness pass (one transaction per job, opened by `job/read/lock.sql`: `mark_scheduled`/`propagate`, then settle; a zero-row verdict rolls the pass back and it re-runs next iteration); the launcher (`claim_launch` and `claim_control` until the cap is reached or nothing is claimable); the zombie and stuck-pending sweeps — each of the last two in its own transaction. Watch transitions are applied as they arrive, outside the passes, each in its own transaction.

### 5.2 Diagram

```mermaid
stateDiagram-v2
  [*] --> none: plan
  none --> scheduled: rule fired (Rust readiness)
  none --> running: advance_segment (fused member)
  none --> upstream_failed: can never fire, failure-like upstream
  none --> upstream_skipped: can never fire, skip-like upstream
  none --> canceled: cancel, or stop-like upstream
  scheduled --> queued: claim_launch (write-ahead), then Job create
  scheduled --> running: claim_control (control task)
  up_for_retry --> running: claim_control (control task)
  scheduled --> none: clear (same row, budget re-based)
  scheduled --> canceled: cancel
  queued --> running: pod start via channel
  queued --> scheduled: pre-start failure (launch budget), or capacity wait (no budget)
  queued --> failed: launch budget exhausted (infrastructure)
  running --> succeeded: finish via channel, watch confirms
  running --> skipped: TaskError::Skip
  running --> failed: failure report, or watch evidence (OOMKilled, no record, zombie)
  running --> timed_out: Job condition Failed/DeadlineExceeded
  running --> scheduled: DisruptionTarget or node gone while running (launch budget)
  failed --> up_for_retry: retryable and try_number <= max_tries (new row)
  up_for_retry --> queued: not_before passed, claim_launch
  up_for_retry --> none: clear (same row, budget re-based)
  queued --> restarting: clear while live
  running --> restarting: clear while live
  restarting --> none: pod gone, complete_restart (new row)
  queued --> canceling: cancel (write-ahead)
  running --> canceling: cancel (write-ahead)
  restarting --> canceling: cancel (write-ahead)
  canceling --> canceled: pod gone, settle_canceled
  succeeded --> none: clear (new row, max_tries re-based)
  failed --> none: clear (new row)
  timed_out --> none: clear (new row)
  canceled --> none: clear (new row)
  skipped --> none: clear (new row)
  upstream_failed --> none: clear (new row)
  upstream_skipped --> none: clear (new row)
```

### 5.3 Terminal taxonomy and trigger rules

Closed sets, declared as `accepted_values` on day one. State classes: success-like `{succeeded}` (with `outcome ∈ {done, no_op}`; exit 10 is a success, `ExitCode::NothingToDo` in [er-errors](../rust/crates/er-errors/src/lib.rs)), skip-like `{skipped, upstream_skipped}`, failure-like `{failed, timed_out, upstream_failed}`, stop-like `{canceled}`. dagr's `abandoned` is dropped — a zombie is `failed` with `error_class = infrastructure, termination_reason = heartbeat_timeout` — and `satisfied-from-prior` is dropped because a cleared run keeps its run id and an upstream success simply stays `succeeded`.

Trigger rules are dagr's three (`docs/arch.md:50-56`), evaluated **in Rust** from the compiled graph only once every upstream is terminal:

| Rule | Fires when | Can never fire → downstream becomes |
|---|---|---|
| `all_succeeded` (default; the only rule a data-dependent node can carry) | every upstream is success-like | `upstream_skipped` if every non-success upstream is skip-like; `canceled` if every non-success upstream is stop-like; `upstream_failed` otherwise |
| `all_terminal` (`finish`) | every upstream is terminal, any class | never propagates |
| `any_failed` (reserved; no consumer in v1) | every upstream terminal and at least one failure-like | `upstream_skipped` (dagr writes `skipped`; here the propagated form, since nothing ran) |

A can-never-fire verdict is itself terminal and cascades in the same pass (dagr `readiness.rs:48-51`), so a failure deadens a chain in one sweep. The evaluator is a table-driven function in `er-dag/src/readiness.rs`, unit-tested row by row; SQL records its verdicts through the guarded `mark_scheduled` and `propagate` statements. Only `all_succeeded` and `all_terminal` have consumers in v1. Branching is in the task (`TaskError::Skip`), never in the graph.

**Rejected alternative.** Readiness as a SQL predicate over an `upstream` jsonb copy inside the claim (airflow-model design). It puts trigger-rule semantics in two places — the compiled graph and a CASE expression — and the can-never-fire cascade needs Rust regardless; `upstream` stays on the row only as the UI's explanation.

**Job settle.** Settle is the last step of the same readiness pass, after propagation and in the same transaction, so it reads the pass's own verdicts: a job settles when no live row is in a non-terminal state (`none | scheduled | queued | running | restarting | canceling | up_for_retry`). A `none` row with every upstream terminal cannot survive a pass — the pass either schedules or propagates it; one that does survive — with the job row locked for the pass, no concurrent clear can manufacture it — is a bug and settles the job `failed`/`internal` (Airflow's deadlock branch, `dagrun.py:1413-1416`). `finish` has then run (it is `all_terminal` and `teardown`, so it runs under `running` and `canceling` alike) and `job/update/settle.sql` settles from the leaves (teardown tasks excluded): `failed` if any leaf is failure-like, `canceled` if any leaf is stop-like and none failure-like, else `succeeded`. The settle check and the cancel sweep are idempotent passes derived from `jobs.state IN ('running', 'canceling')` and the live row states on every loop pass, never from a remembered event, so the first pass after a restart recovers a `running` job whose last task finished before the crash and a `canceling` job whose pods are already gone. The webhook, mail and entity aggregates fire at settle, exactly as today's `dispatch_once_with` tail.

### 5.4 Clear: retry one task without re-running the run

`POST /v1/orgs/{org}/jobs/{job_id}/tasks/{task_id}:clear` (steward+; `include_downstream` default true, the only safe default when `reconcile` deletes membership absent from the live set; optional `resource_class` override). The handler loads `dag_for(jobs.kind)`, refuses with 409 and the node diff if `jobs.structural_fingerprint` differs from the binary's, refuses with 409 under a `canceling` job, refuses with 409 naming the snapshot when the lake head has moved past the run's last pinned `snapshot_end` unless the request carries `force: true` (§9.5; recorded in `audit_log`), and computes the closure — dagr's demand-driven resume with the operator's selection as the seed (`crates/core/src/resume.rs:40-58`): close downward (everything reachable re-runs), then resolve demand upward (each re-running task demands its inputs; an upstream whose `LakeRef` probe says the snapshot still exists is rehydrated from its row; one whose snapshot `lake_maintain` has expired joins the must-run set). The expanded must-run set is shown in the confirm dialog, not refused with a bare 409. Then, with the job row locked (`job/read/lock.sql`), the four `clear` statements and `reopen` run in one transaction (§4.5): terminal rows rotate with a re-based budget, pending rows reset in place with no pin, live rows go to `restarting` and are rotated by the watch once their pod is gone. The existing `POST …/jobs/{job_id}:resume` keeps its route and becomes "clear every live non-succeeded task" — API-compatible and finally meaningful.

### 5.5 Budgets, backoff, escalation

**Task budget**: `max_tries = retries` at planning (default 2, so three tries like `jobs.max_attempts`), eligibility `try_number <= max_tries` on the failing row, clear re-bases `max_tries = previous_try_number + retries` — Airflow's arithmetic verbatim, where `previous_try_number` is `try_number` for a rotated terminal row and `try_number − 1` for a pending row reset in place (`taskinstance.py:526-531`, `:548`). A `<` convention with `max_tries` default 1 and the same clear formula grants `retries − 1` retries after a clear, which is the off-by-one two of the three drafts carried.

**Infrastructure budget**: `launches <= max_launches` (default 3), spent by anything that means the body never ran or was killed by the platform without a verdict: a fatal waiting reason (`ImagePullBackOff, ErrImagePull, CreateContainerConfigError, InvalidImageName`), a Job create the API refuses for a non-capacity reason, a `FailedCreate` that is not a quota message (a missing PriorityClass, a LimitRange or PodSecurity refusal), a Job with no pod and no event after the bound, a pod that vanished while `queued`, a pod that could not reach the control plane, a SIGTERM or SIGKILL with no platform evidence behind it, and — **while running** — a termination carrying the `DisruptionTarget` condition (`TerminationByKubelet` node-pressure eviction, `EvictionByEvictionAPI` drain, `DeletionByTaintManager`, `PreemptionByScheduler`, `DeletionByPodGC`) or a node that disappeared. On spot that is capacity noise, and charging it to `max_tries` lets a preemption-heavy hour burn a tenant's budget without one data failure (dagr ADR 115 §6, carried one step further). Exhausting it fails the try with `infrastructure`, which is retryable on the task budget: a cluster at capacity costs at most 3×3 Jobs per task, never a loop and never a silent success. One consequence of D-DISRUPT worth knowing: on these pools a spot reclaim does **not** evict a `do-not-disrupt` pod (the NodePools set no `terminationGracePeriod`), so the instance is reclaimed under the pod after the two-minute notice, the node goes `NotReady`, and the pod is deleted by the taint manager once the `unreachable` toleration (default 300 s) expires, or by PodGC once the Node object is gone — `DisruptionTarget: DeletionByTaintManager | DeletionByPodGC`; which path Auto Mode takes is unverified and the kind probe cannot answer it, so Phase 5 records it. The watch's "node gone" row covers both, the zombie sweep stands down while the pod shows `NodeNotReady` (§5.7), and the delay of a few minutes is the price of never losing an hour of work to a voluntary consolidation.

**No budget at all — capacity.** `Unschedulable` past the 10-minute bound and every quota refusal return the row to `scheduled` through `capacity_wait` (§4.5) and count in `er_quota_waits_total`, because a starved account is the operator's problem, never the tenant's: a run waits, visibly (§5.7, §10), and never fails for it.

**Which classes retry** (on the task budget): `transient_io`, `lock_conflict` (both already `ErrorClass::retryable()`), `infrastructure`, and `resources` (OOMKilled, or a local ephemeral-storage eviction) **with escalation** — the try never committed, so retrying at the next class is right-sizing on evidence; `escalate_on_oom` is a `NodePolicy` knob, default on for `match` and `reconcile`, and a try already at L is terminal `resources`. Terminal: `config`, `precondition`, `contradiction`, `non_convergence`, `data`, `internal`, `timeout` (a timed-out stage times out again at the same policy; a longer `timeout` is a policy-hash change the operator applies by clearing). The two taxonomies — `pg::FailureClass`'s substring matching and `er_errors::ErrorClass` — collapse into the latter at the executor boundary; `classify` is deleted in Phase 4.

**Backoff**: `min = least(cap, base·2^(try−1))`, `delay = min + hashtext(job#task#try) mod min` — deterministic, so `pg_task_instances.rs` asserts exact deadlines. Base 30 s and cap 900 s for a pod task, because a cold launch costs ~30 s (§8) and today's 5 s base is noise at pod granularity; base 5 s and cap 60 s for a control task; per task overridable through the policy hash.

**Rejected alternative.** OOM terminal, operator clears with a larger class (dagr-first design). For a product whose pitch is cheap right-sizing, automatic S→M→L on an OOM that never committed is the obvious win.

### 5.6 Cancel

`POST …/jobs/{job_id}:cancel` runs `job/update/cancel.sql`: a `queued` job becomes `canceled` directly; a `dispatching | running` job becomes `canceling`, which is in the `jobs_one_active_per_org` predicate, so the org slot stays held until every pod is gone — the trap today, where `canceled` frees the slot while the in-process thread is still writing. The dispatcher's cancel sweep, idempotent on every pass for every `canceling` job, then runs `cancel_pending` (every pending non-teardown row), `cancel_live` (every live non-teardown row → `canceling`, the write-ahead marker), and for each of those rows patches `er.dupezero.com/owner: revoked` and deletes the Job with foreground propagation (dagr's two-step, so the deletion is legible as ours). A control row is marked `canceling` too: its body, if in flight, returns under the dispatcher's own drain, the sweep then runs `settle_canceled` on it (there is no pod to wait for), and its own late failure report is a no-op because `archive_try` requires the job to be `running`. `canceled` on the row is written only by the control plane, from the watch, once the pod is gone (`settle_canceled`); when no live row remains, `finish` runs as teardown and the job settles `canceled`; `job.canceled` is emitted once. The teardown is the one row the cancel sweep never touches: it is admitted under `canceling`, runs, writes `runs.status`, and fails or retries through `archive_try` like any other row.

**What SIGTERM does in the pod.** No engine cancellation flag exists today — `er-cli`, `dedupe-core`'s session, `er-match` and `er-reconcile` carry none, and the dispatcher's drain flag never reaches the engine. Phase 2 adds one at the session level: `dedupe_core::session::CancelToken` wraps an `AtomicBool` and the `Arc<duckdb::InterruptHandle>` from `Connection::interrupt_handle()` (the `duckdb` crate exposes it); `Session::execute_batch`/`prepare` check the flag before issuing and map any DuckDB error that arrives while it is set to a new `DedupeError::Canceled` (`#[error("canceled")]`, no source — the Display test enforces this). `er-server exec-task` runs a `signal-hook` thread on `SIGTERM` that calls `token.cancel()` from a normal thread (the interrupt handle takes a mutex, so never from the signal handler itself). The interrupted statement errors, the open `BEGIN … COMMIT` batch is rolled back on the same connection — DuckLake writes its snapshot only at COMMIT, so the lake is untouched — the session is dropped, and the pod writes an `interrupted` termination record, **posts no verdict** (`note_interruption` only), flushes the tracer with a timeout and exits 3 (no new exit code: `exit_code_for` folds anything outside 0/1/2/3/10 to 1, [er-errors](../rust/crates/er-errors/src/lib.rs)). Kubernetes delivers SIGTERM identically for a cancel, a clear-while-running, a Job deadline, a drain and a node shutdown, so the pod cannot know which it is; the watch decides from the platform's evidence once the pod is terminal (§6.5): `canceling` → `settle_canceled`, `restarting` → `complete_restart`, `DeadlineExceeded` → `timed_out`, `DisruptionTarget` or node gone → `requeue_launch`, none of these (a manual delete, an unrecorded shutdown) → `requeue_launch(sigterm)` on the launch budget. `terminationGracePeriodSeconds: 120`, set here (D-JOBSPEC records no grace): it must exceed the longest single write-back, since a thread inside a DuckDB `MERGE` returns only when the interrupt lands.

### 5.7 Timeouts, zombies, stuck pending

**Timeout**: `NodePolicy::timeout` or the class default (S 2 h, M 6 h, L 24 h, [infrastructure.md](infrastructure.md) §6.2's Job sketch) becomes the Job's `activeDeadlineSeconds`. A Job-level deadline deletes the pod and sets the **Job** condition `Failed` with `reason: DeadlineExceeded`; the pod shows no such reason, and its own `interrupted` record is not a verdict, which is why the watch has a Job stream (§6.5). The row becomes `timed_out`, terminal.

**Zombie**: the watch is the primary liveness signal; the heartbeat covers a process hung inside a pod that is still `Running`. A `running` row with `heartbeat_at < now() − 5 min` and `owner_epoch = mine` is re-validated under `FOR UPDATE SKIP LOCKED`, marked `termination_reason = heartbeat_timeout` as a write-ahead (no state change), and its Job revoked and deleted; when the watch reports the pod gone, `fail_or_retry` runs with `error_class = infrastructure`, charging a try: something ran and the outcome is unknown. The sweep stands down while the pod's `Ready` condition is `False` with reason `NodeNotReady`, or the pod already carries `DisruptionTarget`: the platform is ending it and will deliver the evidence, which lands on the launch budget — a reclaimed spot node is not a zombie. The retry is safe because the ledger probe (§6.3 step 4) matches on the try's retry lineage, so the *n+1* pod reports the zombie's committed output instead of re-executing it.

**Stuck pending**: a `queued` row whose pod has no running container after the pre-start bound — **60 s** for the fatal waiting reasons (dagr `crates/k8s/src/executor.rs:41-56`) and for a Job that has created no pod (§6.5), **10 min** for `PodScheduled=False/Unschedulable`, re-armed when the reason changes. The fatal reasons and a non-quota `FailedCreate` spend a launch; `Unschedulable` and a quota `FailedCreate` spend nothing (`capacity_wait`). Ten minutes, not dagr's one, because NodeClaim → Ready measures 14–18 s but a quota-starved account leaves a pod `Pending` indefinitely. `error_detail` carries the pod's own `PodScheduled=False` message (`0/N nodes are available: … untolerated taint …`) and the newest `FailedScheduling` event message for the pod — where Karpenter writes its "incompatible with NodePool / insufficient capacity" verdicts — followed by a canonical dispatcher-authored sentence for the past-bound case: *no node joined within 10 min for class {S|M|L}; in this account the first suspect is the Standard vCPU quota ([aws-account-operations skill](../.claude/skills/aws-account-operations/SKILL.md))*. The NodeClaim's own launch failure (`VcpuLimitExceeded`, `InsufficientCapacityError`) is recorded on a cluster-scoped object and is not visible from the namespaced Role of §6.7, so the trap surfaces in the UI as a waiting task with that hint rather than as a silent hang, and the diagnosis is the operator's.

## 6. The Kubernetes executor, the only one

### 6.1 Job, not bare Pod

dagr watches bare Pods because it owns retry and refuses cluster retry; its ADR 115 §2 explicitly allows "Jobs pinned to `backoffLimit=0`". With retry owned by Postgres, a Job with `backoffLimit: 0` is a bare pod plus three things Kubernetes does for free: `activeDeadlineSeconds` as a control-plane-independent kill, `ttlSecondsAfterFinished` so a lost `delete` is covered everywhere except the name reuse under `capacity_wait` (§4.5), and the owner reference that garbage-collects the pod. D-JOBSPEC chose Job; this amends it from one Job per run to one per task attempt. `backoffLimit: 0` and `restartPolicy: Never` remain an invariant, not a default (dagr ADR 115 §2).

**Rejected alternative.** `podFailurePolicy` with `action: Ignore` on `DisruptionTarget`, letting the Job controller replace a spot-interrupted pod (k8s-native design). The rule does not create overlapping pods — with any `podFailurePolicy` set, `podReplacementPolicy` defaults to `Failed` and no other value is permitted (GA in Kubernetes 1.34; the cluster is pinned at 1.36 in [eks.tf](../infra/terraform/envs/dev/eks.tf)), so the replacement waits for the old pod to be fully terminal — and that is not the objection. The objection is that it is a second launch path the control plane cannot see: the replacement runs the same argv for the same attempt under the same Job name; for a `running` row `start.sql` refuses it and the pod exits 3 having burned a node, for a `queued` row it is accepted and silently continues the attempt — either way `launches` recorded nothing and the control plane can neither see nor count the relaunch. The control plane relaunches on the infrastructure budget instead — the same "no try spent" outcome, with the write-ahead row, the stale-workload check and the ledger probe guarding it, and `backoffLimit: 0` keeping Kubernetes out of retry entirely (dagr ADR 115 §2).

### 6.2 The per-attempt Job

Rendered by the dispatcher from the Helm-supplied template (§6.8); every load-bearing field annotated.

```yaml
apiVersion: batch/v1
kind: Job
metadata:
  name: er-t-3f9a1c2b7d4e6f8a9b0c-l1           # pure function of the attempt key: 20 hex of task_instances.id + launch count
  namespace: er-dev-alice                        # the platform namespace (D-TENANCY); per developer in dev
  labels:                                        # selectors only (<= 63 chars); lossy by design
    er.dupezero.com/managed: task
    er.dupezero.com/job: "<job_id>"
    er.dupezero.com/task: "init"                 # the segment's first task; this M-class run is one segment (§3.7)
    er.dupezero.com/try: "1"
    er.dupezero.com/class: M
    er.dupezero.com/owner: "<owner_epoch>"       # adoption rewrites; revocation sets 'revoked'
    # er.dupezero.com/done: "true"               # tombstone, patched after the outcome is consumed
  annotations:                                   # authority
    er.dupezero.com/task-instance-id: "<64 hex>"
    er.dupezero.com/task-id: "init"
    er.dupezero.com/structural-fingerprint: "<sha256>"
    er.dupezero.com/policy-hash: "<sha256>"
    er.dupezero.com/image-digest: "sha256:<digest>"
    er.dupezero.com/traceparent: "00-<trace>-<span>-01"
spec:
  backoffLimit: 0                                # INVARIANT: Postgres owns retry (D-JOBSPEC)
  activeDeadlineSeconds: 21600                   # the sum of the segment members' timeouts (NodePolicy::timeout or the class default each); fires as a Job condition
  ttlSecondsAfterFinished: 300                   # Postgres is the record; 300 s covers the watch's 5-min resync
  template:
    metadata:
      labels: { <same labels> }                  # so the Pod watch selects the pod too
      annotations:
        karpenter.sh/do-not-disrupt: "true"      # D-DISRUPT: no voluntary disruption of an hour of DuckDB work
        er.dupezero.com/task-instance-id: "<64 hex>"
    spec:
      restartPolicy: Never                       # INVARIANT
      serviceAccountName: er-runner              # D-TENANCY's SA parameter, realised: Pod Identity, no Kubernetes RBAC
      automountServiceAccountToken: false        # the projected token below is the pod's only token
      terminationGracePeriodSeconds: 120         # SIGTERM -> rollback -> exit 3; must exceed the longest write-back
      priorityClassName: er-task-scheduled       # er-task-interactive for UI "Run now", er-task-maintenance for lake_maintain
      tolerations:
        - { key: er.dupezero.com/runner, operator: Equal, value: "true", effect: NoSchedule }
      nodeSelector: { karpenter.sh/nodepool: runner-sm }    # runner-l for class L
      securityContext: { runAsNonRoot: true, runAsUser: 1001, seccompProfile: { type: RuntimeDefault } }
      containers:
        - name: task
          image: 797781631727.dkr.ecr.us-east-2.amazonaws.com/er-api@sha256:<digest>   # the one multicall image; images.runner is retired
          command: ["er-server", "exec-task"]
          args: ["--attempt", "<64 hex>", "--expect-structural", "<sha256>", "--through", "finish"]   # the whole chain in one pod
          env:
            - { name: ER_CONTROL_URL, value: "http://er-api.er-dev-alice.svc.cluster.local:8000" }
            - { name: ER_DUCKDB_THREADS, value: "6" }          # SAME template operation as requests.cpu (D-SIZING invariant 1)
            - { name: ER_DUCKDB_MEMORY_LIMIT, value: "6GB" }   # strictly below limits.memory (invariant 2)
            - { name: ER_DUCKDB_EXTENSION_DIR, value: /opt/duckdb/extensions }
            - { name: OTEL_EXPORTER_OTLP_ENDPOINT, value: http://otel-collector.observability.svc.cluster.local:4318 }
          resources:
            requests: { cpu: "6", memory: 10Gi, ephemeral-storage: 150Gi }   # class M; the ephemeral request is the anti-stacking lever (D-SPILL)
            limits:   { cpu: "6", memory: 10Gi, ephemeral-storage: 150Gi }   # limits.cpu MUST be set: the LimitRange defaults it to "1" and the quota caps limits.cpu
          terminationMessagePath: /dev/termination-log
          terminationMessagePolicy: File         # the <= 4 KiB terminal record the watch reads from terminated.message
          securityContext: { allowPrivilegeEscalation: false, capabilities: { drop: [ALL] } }
          volumeMounts:
            - { name: duckdb-tmp, mountPath: /app/.tmp }      # DuckDB's CWD-relative spill
            - { name: er-efs, mountPath: /srv/er }             # D-JOBSPEC: configs, drops, models; "fails late if absent"
            - { name: cp-token, mountPath: /var/run/er, readOnly: true }
      volumes:
        - { name: duckdb-tmp, emptyDir: { sizeLimit: 150Gi } }  # per class; lands on Auto Mode's NVMe RAID0 (D-SPILL)
        - { name: er-efs, persistentVolumeClaim: { claimName: er-efs } }
        - name: cp-token
          projected:
            sources:
              - serviceAccountToken: { audience: er-control-plane, expirationSeconds: 3600, path: token }
```

What the spec does **not** contain: the tenant's lake DSN, the control-plane DSN, the operator token, any Secret reference, any `env.value` credential. `limits.cpu` is set equal to the request because the dev namespace `LimitRange` defaults `limits.cpu` to `"1"` and the `ResourceQuota` caps `limits.cpu` ([er-dev-namespace values.yaml](../infra/k8s/charts/er-dev-namespace/values.yaml), [quota.yaml](../infra/k8s/charts/er-dev-namespace/templates/quota.yaml)): a container without one is admitted and throttled to one core. The class table is [infrastructure.md](infrastructure.md) §6.5 — S 2 vCPU / 6 GiB / 4GB / 40 GiB, M 6 / 10 GiB / 6GB / 150 GiB, L 12–16 / 14 GiB / 8GB / 400 GiB — rendered from one `ResourceClass` value into `requests`, `limits`, `ER_DUCKDB_THREADS`, `ER_DUCKDB_MEMORY_LIMIT` and `emptyDir.sizeLimit` in the same operation. L requests 12 vCPU, never 16: a 16-vCPU node's allocatable is capacity minus kube-reserved and the DaemonSets' requests, roughly 14–15 cores, so a 16-core request can never place. That fixes the filed throughput bug ([research-runner-startup.md](research-runner-startup.md): every dispatched job single-threaded with no memory limit) — subject to the determinism gate in §9.3.

### 6.3 The pod-side verb: `er-server exec-task`

A subcommand of `er-server`, beside `migrate`, `migrate-tenants` and `install-extensions` ([main.rs](../rust/crates/er-server/src/main.rs) dispatches those on `args[1]` after the `argv[0]` switch). It cannot be an `er` subcommand: that face hands everything to `er_cli::cli::run()`, whose clap parser has a closed `Command` enum and required lake globals, and the verb needs `dag_for` from `er-server/src/dags/`, which `er-cli` cannot see. It is synchronous — the stage runs on the main thread, the four HTTP calls on a current-thread runtime, the heartbeat and the SIGTERM handler on plain threads.

1. Read the projected token; `GET {ER_CONTROL_URL}/v1/internal/attempts/{id}` with it. The response is the **work order**: the task name and try, `input` (the `RunPlan`, upstream `LakeRef`s, params), the resolved class, the segment bounds, the try's **retry lineage** (`task_instance/read/retry_lineage.sql`: this id plus the archived `retry` predecessors of `(job_id, task_id)`, walked back to the first `cleared`/`restarted` row), and the tenant's lake env from `orgs.env` (`ER_LAKE_CATALOG`, `ER_LAKE_DATA_PATH`, `ER_LAKE_METADATA_SCHEMA`, `ER_CONFIG_PATH`, drop root, model and lexicon paths). Unreachable for 60 s → termination record `control_plane_unreachable`, exit 3; the watch charges a launch.
2. `dag_for(kind)` from the binary's own registry; compare `--expect-structural`; mismatch → termination record `structural_mismatch`, **exit 3** (`ExitCode::Precondition` — this repo's exit table has no `ResumeRefusal`). Nothing about the pipeline crossed the wire, so dispatcher/pod skew is impossible by construction (dagr `crates/cli/src/exec_node.rs:248-250`).
3. `POST …/start {node_name, image_digest}` — `pod_name` and `pod_uid` come from the token's claims, never from the body. Zero rows → record `stale_start`, exit 3 after logging why. Start the heartbeat thread (30 s) and the SIGTERM thread (§5.6). Open the session with `SessionOptions` from `ER_DUCKDB_THREADS`/`ER_DUCKDB_MEMORY_LIMIT`; `open_lake`; for `init`/`reconcile_schema`, run the lake reconcile and report the tenant schema state.
4. **Ledger probe** (`run_stage/read/succeeded_in_lineage.sql`): a `succeeded` `run_stages` row for `(run_id, stage)` whose `task_instance_id` is in this try's retry lineage means a disrupted or zombie predecessor committed and died before reporting → `POST finish` for this try from the ledger row (snapshots, counters, outcome; the ledger row's `task_instance_id` recorded as `counters.adopted_from`) and exit 0. Never re-execute committed work. A `cleared`/`restarted` predecessor is a lineage boundary, so a clear still re-runs.
5. Check arity and rehydrate inputs positionally, probing each `LakeRef` through `Prober::snapshot_exists` (`Missing` → `precondition`). Write `run_stages` start. Run exactly one try with no retry, backoff or timeout of its own (dagr `exec_node.rs:22-28`).
6. Before the last write-back, check the fence flag (a heartbeat that returned zero rows, or three consecutive failures); if fenced, roll back, record `fenced`, exit 3. Otherwise commit, write `run_stages` finish as the last dedicated commit, read `snapshot_end`.
7. Write the ≤ 4 KiB JSON **terminal record** `{v, attempt, state, exit, error_class, termination_reason, detail, output, snapshot_start, snapshot_end, counters}` to `/dev/termination-log` on **every deliberate exit path** — steps 1, 2, 3 and 6 and §5.6's SIGTERM included, `detail` truncated so the JSON always parses inside the kubelet's 4096-byte cap — and `POST …/finish` with the same record. `state` is `succeeded | skipped | failed | interrupted`; the handler runs `finish.sql`, `fail_or_retry` or `note_interruption.sql` by it. `force_flush` the tracer with a timeout on every exit path ([infrastructure.md](infrastructure.md) §10). Exit with the `er-errors` code: 0, 1, 2, 3 or 10.
8. For a segment (§3.7), *k*'s `finish` carries `next`; the handler's `advance_segment` moves *k+1* to `running` in the same transaction and the response says so, and the pod loops from step 4 for *k+1* keeping the session. "No advance" → exit 0 after *k*. Exit at the first failure.

### 6.4 The report channel and its credential

**D-PROGRESS amended.** The runner still originates its own progress, but through four routes on `er-server` — `GET /v1/internal/attempts/{id}`, `POST …/start`, `POST …/heartbeat`, `POST …/finish` (which also carries failure and interruption) — never with a database credential. The security argument is specific to this product: the pod evaluates tenant-authored raw SQL (`blocking[].expr`, `training.*_rules`, an authenticated code-execution surface per the [security skill](../.claude/skills/security/SKILL.md)) in a DuckDB with `postgres_scanner` baked in ([Dockerfile](../rust/Dockerfile)), so it must hold neither `ER_PG_DSN` (every tenant's control-plane state) nor `ER_OPERATOR_TOKEN` (cross-org). It does hold the tenant's own catalog DSN — the data plane, as today, which D-TENANCY accepts. The routes are reachable only in-namespace: the existing `default-deny-ingress` NetworkPolicy allows same-namespace pods, the `er-api` Service is ClusterIP on no load balancer ([services.yaml](../infra/k8s/charts/er-platform/templates/services.yaml)), and the BFF never forwards `/v1/internal/*` — a frontend gate beside `check-bff-authz.mjs` asserts no route under `src/app/api` names that prefix. The token is redacted from request logs. Every DTO is `#[serde(deny_unknown_fields)]`; `er.actor = task:<id>` on every write.

**The credential is the projected, audience-bound ServiceAccount token** Kubernetes mints for the pod (`serviceAccountToken` projected volume, `audience: er-control-plane`, §6.2). `er-server` verifies it offline against the cluster's JSON Web Key Set, checks `aud`, and binds the attempt by the `kubernetes.io/pod/name` claim: the pod name's Job-name prefix is a pure function of the attempt key, so the server derives the expected prefix from `task_instances.k8s_job_name` and refuses any other pod. At `start` it records `pod_name` and `pod_uid` from the token's `kubernetes.io/pod/{name,uid}` claims and the watch cross-checks them later — which closes the race where the pod calls before the watch has delivered its UID. Nothing secret is stored in etcd, no `secrets` RBAC exists, and Kubernetes proves which pod is speaking. This is Airflow 3's "workers never hold DB credentials" (`core/executors/workloads/base.py:112-126`) with the identity proven by the platform rather than by a JWT we mint.

**Where the key set comes from.** `er-api` reads it from the in-cluster API server at the fixed path `https://kubernetes.default.svc/openid/v1/jwks` — never by following the discovery document's `jwks_uri`, which on EKS names the public issuer's keys URL (the API server runs with `--service-account-jwks-uri` pointing there; that endpoint is throttled and rotates every seven days) — authenticating with its **own** projected ServiceAccount token (the default API-server audience, `expirationSeconds: 3600`), mounted as a projected volume with the three standard sources (`serviceAccountToken`, `kube-root-ca.crt`, the downward-API `namespace`) at the default path so `kube::Config::incluster()` works unchanged. That endpoint is RBAC-protected, and the only grant `er-api` uses is the default `system:service-account-issuer-discovery` ClusterRoleBinding on `system:serviceaccounts` (beside the `system:basic-user` and `system:discovery` defaults every ServiceAccount inherits); it gets no Role or RoleBinding of its own and keeps `automountServiceAccountToken: false` — the volume is explicit. The mechanism is identical on kind and EKS, needs no outbound fetch, no issuer URL to pin, and no trust in an unverified `iss` claim. The cache refreshes on a timer and once on an unknown `kid` (key rotation), with a bounded clock-skew tolerance on `exp`/`nbf`. `tests/egress_guard.rs` is not a factor either way: it bans `reqwest::Client::new()` and the unguarded webhook seam, nothing else.

**Rejected alternatives.** A per-attempt random token in `env.value` (airflow-model design): readable by anyone with `pods get` or `jobs get` in the namespace. Fetching the public EKS issuer's JWKS (`https://oidc.eks.<region>.amazonaws.com/id/<id>/keys`) over egress: kind has no public issuer, so it would be a second, EKS-only path; it needs a Terraform output, a chart value and an environment variable that do not exist; and it makes first-contact verification depend on the internet. `TokenReview`: needs a cluster-scoped binding on `er-api`, which §6.7 excludes. A per-attempt `Secret` with an `ownerReference` to the Job (k8s-native design): needs `secrets create` RBAC on the dispatcher and puts a credential in etcd. The Secret is the **recorded fallback** if the Phase 2 spike fails: then the token is a random 256-bit value, SHA-256 at rest on the row, compared with `ct_eq` like API keys, nulled at finish.

### 6.5 The watch, resync and classification

The dispatcher process owns exactly one watcher on **two streams** under one selector `er.dupezero.com/managed=task,er.dupezero.com/done!=true` — `batch/v1 Jobs` for lifecycle (`Complete`; `Failed` with `DeadlineExceeded` or `BackoffLimitExceeded`; `status.active`) and `Pods` for evidence (phase, `containerStatuses[].state.terminated.{reason,exitCode,message}`, `conditions[DisruptionTarget]`, `conditions[PodScheduled]`, `waiting.reason`, `nodeName`). Two connections per process, not two per task; a pods-only watch cannot see a Job deadline fire or a Job that never got a pod. The core is a sans-IO state machine in a new `er-k8s` crate — a source-vendored, MIT-attributed adaptation of dagr's `crates/k8s/src/observer.rs` (dagr is MIT-licensed) fed `(now, event)` and emitting `(task_instance_id, Transition)` — with the four termination classes and their answers: a decoded `410 Expired` → re-LIST, reconcile, watch from the listing's version; transport end → resume from the last seen version; `403/5xx` → bounded backoff 250 ms doubling to 30 s, 12 failures or 5 min; silence → a 90 s stall bound forces a LIST (dagr `observer.rs:34-39`, `:154-164`). A 5-minute resync LISTs both kinds and reconciles every live row — that loop is also adoption. The waiter for an attempt is registered before the create call, so no transition lands in the gap. Events are not a third stream: the namespace's Events are `list`ed on demand (§6.7) for a Job with no pod and for the pending-age hint.

**Create outcomes.** The create is idempotent on the Job name, which `claim_launch` wrote ahead. `403 … exceeded quota` → `capacity_wait`. `429`, or `503` carrying `Retry-After` → requeue honouring `Retry-After` on the launch budget. Any outcome under which the server may have persisted the Job — `409 AlreadyExists`, a transport error or client timeout, `500/502/504` — is resolved by a `GET` of the Job by name before anything is written: a Job whose `task-instance-id` annotation equals the row's id and whose `structural-fingerprint` equals this binary's, with no `done` label and no `deletionTimestamp`, is ours → adopt it (patch `owner` to this epoch if it differs; the waiter is already registered; no launch charged — `claim_launch` counted this one); `404` → the create did not land, retry it under the same name; anything else under that name (tombstoned, being deleted, foreign fingerprint, wrong annotation) → never adopt and never reuse the name: `requeue_launch`, so the next claim mints `-l(n+1)` and the stale object is left to TTL (a foreign fingerprint additionally follows §6.6's "build skew"). A `409` that is not `AlreadyExists` means the write was refused → `requeue_launch`. Probe-then-adopt is dagr's `launch_once` (`crates/cli/src/k8s_runner.rs:643-681`); its tombstone-revoke-recreate branch is not carried over because the launch-count suffix already makes every name unique. (Airflow retries only the optimistic-concurrency `409 "object has been modified"` on a patch, `kubernetes_executor.py:771`, which cannot arise on a create.)

**Pre-start classification** (`phase == Pending`, or no pod at all). The evidence is the pod's phase, conditions and container `waiting.reason`, plus the namespace's `FailedCreate` and `FailedScheduling` Events:

| Evidence | Verdict | Budget |
|---|---|---|
| container `waiting.reason` ∈ `{ImagePullBackOff, ErrImagePull, CreateContainerConfigError, InvalidImageName}` for 60 s | delete the Job; `requeue_launch(<reason>)` | launch |
| `PodScheduled=False`, `reason: Unschedulable` for 10 min | delete the Job; `capacity_wait(Unschedulable)`; `error_detail` per §5.7 | none |
| Job `status.active = 0`, no pod with its name prefix 60 s after the create, newest `FailedCreate` event says `exceeded quota` | delete the Job; `capacity_wait(quota)`, the quota line in `error_detail` | none |
| same, any other `FailedCreate` message (no such PriorityClass, LimitRange, PodSecurity, invalid spec) | delete the Job; `requeue_launch(FailedCreate)`, message in `error_detail` | launch |
| same, no `FailedCreate` event | delete the Job; `requeue_launch(no_pod_created)` | launch |
| pod gone while the row is `queued` (watch `DELETED`, not ours) | `requeue_launch(vanished)` | launch |

`exceeded quota` is the one deliberate message-text match in the design (the Event carries only the message); a fixture pins it. The same rules run on every 5-minute resync and at adoption, which closes the "Job with zero pods" case of §6.6.

**Terminal classification** (pure; every row has a test). Precedence first, in three clauses: (a) a `finish` report that landed outranks every later pod evidence — the pod posts it only after its last lake commit and the `run_stages` ledger row, so later evidence annotates (`record_evidence`), never revokes; (b) pod evidence outranks the termination message only when no report landed, and a try charged that way is reconciled by the next pod's ledger probe (§6.3 step 4), never by re-execution — the dispatcher opens no lake and cannot consult the ledger itself; (c) for a fused segment (§3.7), pod-level evidence attaches to the member whose row is still `running`, the one with no landed report. And (d): a verdict that ends in a relaunch — `archive_try` on a `running` row, `requeue_launch`, `settle_canceled`, `complete_restart` — is written only on terminal-phase or `DELETED` evidence for that `pod_uid`, never while the pod object is still non-terminal: a foreground Job delete returns when the `deletionTimestamp` is set, the pod then has its 120 s grace, and a process stalled on EFS I/O can outlive even SIGKILL, so the row keeps the org slot until the pod is provably gone. Then, by row state: a `canceling` row → `settle_canceled` on any terminal evidence; a `restarting` row → `complete_restart` on any terminal evidence (recorded, never classified); otherwise, for a `queued` or `running` row:

| Evidence | Verdict | Budget |
|---|---|---|
| pod `Succeeded` and a `finish` report landed | already `succeeded`/`skipped`; tombstone | — |
| pod `Succeeded`, no report, termination message present | classify from the message; a message claiming success → `succeeded` from the message | — |
| pod terminal with the binary's own exit code (`1/2/3/10`) and a `finish` report or termination message present | class, outcome and snapshots from the record; `exit_code` kept as evidence only | per class |
| pod terminal, no report, no termination message after 60 s | row `running` → `failed`, `internal`, `termination_reason = no_terminal_record`: never a success (exit 10 makes the phase `Failed`, so "nothing to do" without a record is not provable) and never a retryable class, because `ErrorClass::exit_code` is many-to-one (`lock_conflict` and `precondition` both exit 3, `transient_io` and `data` both exit 1); row `queued` → `requeue_launch(no_terminal_record)` | try / launch |
| termination record `interrupted` (SIGTERM, rolled back, no verdict) | defer to the platform evidence once the pod is terminal: Job `Failed/DeadlineExceeded` → `timed_out`; `DisruptionTarget` or node gone → `requeue_launch(<reason>)`; neither → `requeue_launch(sigterm)` | try / launch |
| pod `Failed` after a `finish` report landed | no verdict; `record_evidence` stores `exit_code`/`termination_reason`; `er_task_post_finish_failures_total{termination_reason}`; tombstone | — |
| container `terminated.reason = OOMKilled` | `failed`, `resources`, escalate class | try |
| pod `status.reason = Evicted` with **no** `DisruptionTarget` (the kubelet's local ephemeral-storage limit: `emptyDir.sizeLimit` or the container's `ephemeral-storage` limit exceeded; the message names the volume) | `failed`, `resources`, `error_detail` names the volume; escalate | try |
| `DisruptionTarget` condition — `TerminationByKubelet` (node-pressure eviction, which also sets `status.reason = Evicted`), `EvictionByEvictionAPI`, `DeletionByTaintManager`, `PreemptionByScheduler`, `DeletionByPodGC` — while `running` | `requeue_launch(<reason>)`; the message in `termination_reason` | launch |
| **node gone** — pod `DELETED` while `running` without our `revoked` label and with no `finish` report: `DisruptionTarget: DeletionByTaintManager \| DeletionByPodGC`, `status.reason = NodeLost`, or none of these. The Role cannot see nodes (§6.7), so this is always pod evidence | `requeue_launch(<reason>)`, `vanished` when no reason is present | launch |
| termination message `control_plane_unreachable` (exit 3 before `start`) | `requeue_launch`; no try ran | launch |
| pod whose `task-instance-id` row has a different `k8s_job_name` | stale launch: tombstone, no verdict | — |
| Job condition `Failed`, `reason: DeadlineExceeded` | `timed_out`, terminal | try |
| exit 137 with no `OOMKilled` reason and no `DisruptionTarget` | `requeue_launch(sigkill)` — never inferred as OOM (dagr `executor.rs:26-32`) | launch |
| deleted with the `owner: revoked` label | ours: the row is `canceling` or `restarting`, and the state rule above applies | — |

Eviction is classified on the `DisruptionTarget` reason together with `status.reason` and `status.message`, never on the presence of the condition alone: since Kubernetes 1.31 the kubelet's threshold-driven node-pressure eviction sets `DisruptionTarget/TerminationByKubelet` and is a platform event, while its local-storage-limit eviction sets no condition and is this task's spill exceeding its class.

**No `pods/log`.** The termination message carries the terminal record and needs no RBAC; Fluent Bit ships the logs. [infrastructure.md](infrastructure.md) line 202's `pods/log: get` grant is amended out of the Role.

### 6.6 Adoption, tombstone, revocation on restart

The dispatcher is one replica (`Recreate`), so adoption is "the new process owns every live row", not relabelling between schedulers. On start: mint `owner_epoch`; `job/update/adopt.sql` over every `running | canceling` job, then `task_instance/update/adopt.sql` over every live row; LIST Jobs and Pods by the selector; then, per live row, by the **job's** state first. Under a `running` job: a Job whose name equals the row's `k8s_job_name` (the annotation names the segment head; a fused member's row carries the head's Job name through `advance_segment`) and whose `structural-fingerprint` equals this binary's → patch `owner` to the new epoch and resume waiting (never resubmit: a duplicate attempt is two lake writers; an image-digest-only difference is adopted because the contract is the fingerprint and the pod reports through versioned routes); a `queued` row with no Job → the write-ahead record says a launch was intended, so the §6.5 create rule runs under the same name; a `running` row whose Job is gone → `requeue_launch(vanished)`; a Job with zero pods → the §6.5 pre-start rules. Under a `canceling` job: a present Job → revoke-then-delete; an absent one → `settle_canceled` (never a relaunch of a job being canceled). A `restarting` row whose Job is absent — never created, or already gone — → `complete_restart` with `termination_reason = no_pod` and a NULL exit code, on the start-up pass and on every resync; otherwise it would hold the org slot forever. Whatever the job: a Job whose row is terminal → tombstone and let TTL delete it; a Job whose fingerprint differs → **foreign**: tombstoned, never attributed, its row `failed/internal "build skew"` (dagr `crates/k8s/src/adoption.rs:251-284`); a `running` control row → `fail_or_retry(infrastructure, dispatcher_restart)` at once, because `Recreate` guarantees the old process finished or died (§3.6). After any outcome is consumed, patch `er.dupezero.com/done=true` on the Job and pod before relying on TTL, so a finished attempt is never attributed twice even if the delete is lost (dagr `cli/adoption.rs:374-384`). Revocation is always patch-then-delete. TTL is a backstop with a known shape: if `er-api` and the dispatcher are both down for longer than it, a finished pod's record is gone with its Job, the row reads `running` with no Job, `requeue_launch(vanished)` relaunches it, and the ledger probe reports the committed stage instead of re-running it.

**Rejected alternative.** A `dispatcher_lease` table (airflow-model design). With `replicas: 1` and `Recreate` there is one owner by construction; the epoch value on rows and labels is enough, and the SKIP LOCKED claims plus expected-state guards already make an accidental second dispatcher safe.

### 6.7 RBAC, ServiceAccounts, Terraform

```yaml
# templates/rbac-dispatcher.yaml — one namespaced Role, never a ClusterRole (D-TENANCY's cluster-wide-create argument)
apiVersion: rbac.authorization.k8s.io/v1
kind: Role
metadata: { name: er-dispatcher, namespace: {{ .Release.Namespace }} }
rules:
  - apiGroups: ["batch"]; resources: ["jobs"];   verbs: [create, get, list, watch, patch, delete]
  - apiGroups: [""];      resources: ["pods"];   verbs: [get, list, watch, patch]   # patch = labels-only merge patches
  - apiGroups: [""];      resources: ["events"]; verbs: [list]                      # FailedCreate / FailedScheduling messages, on demand
```

Deliberately absent, each load-bearing: `update` (races `status`), `deletecollection`, `pods/log`, `pods/exec`, `events` `get`/`watch`/`create` (the list is read on demand for a Job with no pod and for the pending-age hint; a third watch stream is not worth its own reconnect bug), `nodes`, `secrets`, `configmaps`, `cronjobs`, `tokenreviews`, `karpenter.sh/nodeclaims` (cluster-scoped, §5.7), anything cluster-scoped. A `403` is classified into a sentence naming the missing verb.

Three ServiceAccounts: `er-api` keeps `automountServiceAccountToken: false` ([serviceaccounts.yaml](../infra/k8s/charts/er-platform/templates/serviceaccounts.yaml)) and holds only the explicit projected API-server-audience token of §6.4, with no Role of its own, used solely to read the issuer's JWKS; it never holds Job-create (the file's comment "nothing in the pod talks to the Kubernetes API" is rewritten when this lands). **`er-dispatcher`** (new, automount true, bound to the Role) runs the dispatcher as its own Deployment (`replicas: 1`, `Recreate`, the existing `er-dispatcher` multicall arm with its SIGTERM drain, [dispatcher_bin.rs](../rust/crates/er-server/src/dispatcher_bin.rs)); **`er-runner`** (new, automount false, no RBAC) is the pod's identity, with an EKS Pod Identity association in Terraform — `aws_eks_pod_identity_association` per namespace in a new `er_runner_pod_namespaces` list beside `er_api_pod_namespaces` ([pod_identity.tf](../infra/terraform/envs/dev/pod_identity.tf)), binding to a new `er-dev-runner-pod` role that carries the lake-prefix S3 policy. The role exists for when the S3 lake is openable; today the Rust engine cannot open an `s3://` lake at all ([ducklake-lakehouse skill](../.claude/skills/ducklake-lakehouse/SKILL.md)), so EFS at `/srv/er` remains the data path the executor inherits.

**Rejected alternative.** Keeping the dispatcher in the merged `web-api` pod and flipping `er-api`'s automount on (k8s-native and dagr-first designs). The Role that creates Jobs — arbitrary code in the namespace under the runner's AWS identity — must not sit on the pod that serves the API behind the BFF. The split is a chart change, not a code change, and the dispatcher pod mounts EFS for the control tasks that read a tenant's config. Note the decoupling of deploys from runs comes from the Kubernetes Job outliving any dispatcher pod plus adoption, not from the split. Also rejected: a read-only ClusterRole on `karpenter.sh/nodeclaims` to attribute a stuck pod to its NodeClaim's launch failure — cluster-scoped, outside the per-namespace chart, and correlated only by time.

### 6.8 Helm values and templates

Chart `er-platform`: `deployment-dispatcher.yaml` (new), `rbac-dispatcher.yaml` (new), `serviceaccounts.yaml` (+`er-dispatcher`, +`er-runner`), `configmap-task-template.yaml` (the Job template of §6.2 as a Go-templated YAML the dispatcher renders per attempt — the chart is the only place the Job template lives; mounted into the dispatcher pod as a volume at a fixed path and never read through the API, which is why `configmaps` stays out of the Role), values `executor.inFlightCap` (default 1), `executor.ttlSecondsAfterFinished` (300), `executor.classes.{S,M,L}` (the class table), `executor.priorityClass.{interactive,scheduled,maintenance}`, `executor.dagKinds` (the rollout allowlist, deleted in Phase 4), `executor.quotaWaitSeconds` (30); `deployment-web-api.yaml` gains the read-only projected API-server-audience token volume on the `er-server` container (§6.4), not mounted into `er-web`. The `dispatcher.launcher` / `images.runner` "NOT IMPLEMENTED" blocks are deleted. Cluster-scoped objects — the three `PriorityClass`es — cannot live in a per-namespace chart; they are applied beside the NodePools from `infra/k8s/priorityclasses/*.yaml` by `make nodepools` (which becomes `make cluster-objects`). Chart `er-dev-namespace`: `quota.jobs` raised per §7.2.

### 6.9 The fake pod API is a test double, not a second executor

`er-k8s` exposes one trait the dispatcher launches and watches through:

```rust
pub trait PodApi: Send + Sync {
    fn create_job(&self, spec: &JobManifest) -> BoxFuture<'_, Result<Created, ApiError>>;
    fn get_job(&self, name: &str) -> BoxFuture<'_, Result<Option<JobSnapshot>, ApiError>>;      // the §6.5 create-outcome probe
    fn list_events(&self, about: &ObjectRef) -> BoxFuture<'_, Result<Vec<EventSnapshot>, ApiError>>; // FailedCreate / FailedScheduling, on demand
    fn list(&self, selector: &Selector) -> BoxFuture<'_, Result<Listing, ApiError>>;
    fn patch_labels(&self, kind: Kind, name: &str, labels: &[(&str, Option<&str>)]) -> BoxFuture<'_, Result<(), ApiError>>;
    fn delete_job(&self, name: &str) -> BoxFuture<'_, Result<(), ApiError>>;
    fn watch(&self, selector: &Selector, from: Option<ResourceVersion>) -> BoxStream<'_, WatchFrame>;
}
```

`KubeApi` over `kube` is the only production implementation, always compiled (no Cargo feature) so `make check`'s clippy and doc cover the executor path. `er-server/tests/support/fake_pod_api.rs` is a **test double of the Kubernetes API**: on `create_job` it runs `er-server exec-task` as a child process with the manifest's argv and env (the token volume replaced by the fallback token for the test), and synthesises the `WatchFrame`s a cluster would emit — `Pending → Running → Succeeded|Failed` with the child's real exit code and termination file, plus scripted `OOMKilled`, `DisruptionTarget`, `Unschedulable`, `FailedCreate`, `410 Expired` and stalls for the classifier and observer suites; its `get_job` and `list_events` answers are scripted the same way. It has no scheduling, retry, readiness or classification; all of that is the real dispatcher code against fake events. It lives under `tests/`, is never linked into the binary, and there is no `ER_EXECUTOR` knob: `tests/no_second_executor.rs` fails if the double's type name or `ER_EXECUTOR` appears outside `tests/`.

### 6.10 The local developer story

`make check` runs the readiness table, assembly refusals, classifier and observer suites, SQL plans and the child-process double in-memory, plus the `pg_*` tier when `ER_TEST_PG_DSN` is set (one `#[tokio::test]` per file, per `tests/pg_serial.rs`). `make check-e2e` adds `pg_run_all_dag.rs`, which drives a full `run_all` through the double against a DuckLake. `make kind-up` brings up kind with a local registry and the chart pointed at it (`runner.nodeSelector: {}`; the data-path volume is a chart value, `runner.dataVolume` — the `er-efs` PVC on EKS, a `hostPath` under the kind node's mount on kind — and `er-runner` carries no AWS identity on kind and needs none, since the lake is a directory under that path); `make check-kind` runs five phases there — `lake_maintain` end to end, adoption across a dispatcher restart, cancel, a forged token refused, and (since Phase 3) `run_all_full` as seven pods — a CI job on `workflow_dispatch` and nightly, not a required check until it has been observed failing ([docs/frontend-testing.md](frontend-testing.md)'s rule for every gate). The pipeline phase is the one that can fail for a reason no single-task kind reaches: a stage reading what the *previous stage's process* wrote. `seed_lexicon`'s session-local table is the worked example — invisible while one process ran every stage, fatal the moment `init` and `standardize` were separate pods. It uses its own org, because the other four phases depend on `lake_maintain` settling in seconds against an empty lake. The dev namespace in the real cluster is the final probe, and it is still the only place the class boundary is observable: kind has one node, so the S→L split and the two-node-class assertion cannot be made there at all. `make run` stays for API-only work and loses `ER_DISPATCH_IN_PROCESS` in Phase 4.

### 6.11 Control flow for one attempt

```mermaid
sequenceDiagram
  participant D as er-dispatcher
  participant P as Postgres
  participant K as Kubernetes API
  participant Pod as er-server exec-task
  participant A as er-api
  D->>P: one transaction: claim.sql (org slot) · plan.sql rows · job -> running
  D->>P: readiness pass: mark_scheduled / propagate / settle
  D->>P: claim_launch (write-ahead: queued, launches+1, k8s_job_name, owner_epoch)
  D->>P: re-read: state=queued AND try AND owner_epoch
  D->>K: create Job (backoffLimit 0, deadline, TTL, class requests)
  K-->>D: watch: Pod Pending -> Running
  Pod->>A: GET /v1/internal/attempts/{id} (projected SA token, aud er-control-plane)
  A->>K: JWKS from /openid/v1/jwks (er-api's own projected token; cached)
  A->>P: bind the pod-name prefix to k8s_job_name
  A-->>Pod: work order (plan, LakeRefs, retry lineage, class, tenant lake env)
  Pod->>A: POST start
  A->>P: start.sql (queued -> running; pod_name/pod_uid from the token's claims)
  loop every 30 s
    Pod->>A: POST heartbeat
    A->>P: heartbeat.sql (zero rows => fence)
  end
  Pod->>Pod: dag_for(kind) · --expect-structural check
  Pod->>Pod: ledger probe by lineage · rehydrate inputs · run_stages start · run stage · fence check · commit · run_stages finish (last commit)
  Pod->>A: POST finish (snapshot_end, output, counters, next)
  A->>P: finish.sql (running -> succeeded) [+ advance_segment for next, same transaction]
  Pod->>K: exit 0 (+ /dev/termination-log record)
  K-->>D: watch: Job Complete, Pod Succeeded
  D->>K: patch done=true (tombstone); TTL deletes the Job
  D->>P: readiness pass: downstream -> scheduled, input gains the LakeRef pin
```

## 7. What Kubernetes owns

### 7.1 Delegated, by field

| Concern | Kubernetes field or object | What the control plane still does |
|---|---|---|
| placement, bin-packing, node provisioning, spot/on-demand choice | `resources.requests.{cpu,memory,ephemeral-storage}` per class; `nodeSelector: karpenter.sh/nodepool`; the runner taint + toleration; the `karpenter.sh/v1` NodePools under [infra/k8s/nodepools/](../infra/k8s/nodepools/) | chooses the class per task; nothing else |
| not stacking two spill-heavy tasks on one node | `requests.ephemeral-storage` (D-SPILL) | — |
| no eviction of an hour of DuckDB work | `karpenter.sh/do-not-disrupt` on the pod; pools `WhenEmpty` (D-DISRUPT) | — |
| scale to zero and the warm window | `consolidateAfter: 10m`, `expireAfter: 72h` | — |
| the attempt deadline, even if er-server is down | `activeDeadlineSeconds` per task | records `timed_out` from the Job condition |
| cleanup | `ttlSecondsAfterFinished: 300`; Job → Pod owner reference | tombstones before relying on TTL; Postgres is the record |
| ordering under contention | three `PriorityClass` objects (`er-task-interactive` > `er-task-scheduled` > `er-task-maintenance`), all `preemptionPolicy: Never` — priority orders the pending queue by who is waiting and never kills | maps the job's trigger (UI "Run now", schedule, system) to the class |
| namespace ceilings | `ResourceQuota` (memory, `limits.cpu`, ephemeral storage, `count/jobs.batch`) + `LimitRange` defaults for anything that forgets a request | sizes the quota from the in-flight cap (§7.2) |
| spot interruption, drain, node loss | the `DisruptionTarget` condition and its reason, or the node's disappearance, as evidence | relaunches on the infrastructure budget |
| pod identity and AWS identity | projected audience-bound SA token; EKS Pod Identity on `er-runner` (never IRSA) | verifies the token; binds the attempt |
| NVMe spill | Auto Mode's instance-store RAID0 under `emptyDir` | sets `sizeLimit` per class |
| logs and traces transport | Fluent Bit DaemonSet → CloudWatch; the OTel collector | deep-links by pod name; never tails |

**Not used, and why:** `podFailurePolicy: Ignore` (a second launch path, §6.1); `spec.suspend` (a pre-staged Job holds a `count/jobs.batch` slot for nothing, and the write-ahead row plus a pure-function Job name already close the create-then-record gap through adoption — `suspend` is mentioned once as the Kueue adoption seam); `schedulingGates` (an external admission controller's tool); `completionMode: Indexed` (no fan-out); `successPolicy`; `topologySpreadConstraints` (we want packing, not spreading).

**Kept in the control plane, because Kubernetes has no primitive for it:** *when* a run happens (schedules, enqueue-on-upload, the tenant sweep — dagr: "something outside decides when"); ordering within a run (the edges live in the binary; Kubernetes has no "after" and we refuse to encode the graph twice); the per-tenant single writer (two indexes; no Kubernetes object expresses "one per org"); class-based retry with two budgets (`podFailurePolicy` sees exit codes and conditions, not whether a `data` failure is retryable or whether this try already escalated); the state of record (the UI, webhooks and mail read Postgres, never the API server).

### 7.2 Priority, quotas, the in-flight cap

**In-flight cap.** Pending pods *are* a queue, but a Pending pod holds nothing and tells us nothing until its bound expires. `claim_launch` is therefore gated by `executor.inFlightCap`, a count of `queued | running` pod rows across orgs. **It is 3**, since the Standard vCPU quotas were granted at 64 — On-Demand `L-1216C47A` and Spot `L-34B43A08`, both read back from the account rather than taken on trust, because an earlier measurement recorded 8 and that was the pre-grant number. Three is what the worst case affords: three L pods land on three 16-vCPU nodes, so 3×16 + 4 for app and system is 52 of 64, while four would need 68 and the fourth node could not provision at all. A pod that cannot provision sits `Unschedulable` for ten minutes, costs a launch, and comes back — which is what the cap exists to prevent; `scheduled` rows waiting on it are visible in the UI as exactly that.

At 8 vCPU none of this was reachable: no runner node of either class could provision, and a lingering idle `runner-sm` node blocked a `runner-l` for ten minutes, which is why the class-boundary split was unobservable in this account for the whole of Phase 2.

**The namespace quota binds before the cap does, deliberately.** A dev namespace's `limits.cpu` and `jobs.batch` are ceilings that cost nothing until used, and both are sized past the cap. `requests.ephemeral-storage` is not: three L pods want 1200Gi of spill against a 900Gi quota, so a dev namespace admits **two** concurrent L pods and refuses the third. That refusal is a capacity `FailedCreate`, so it costs `quotaWaitSeconds` and not a launch — the backstop behaving as designed. Raising it is EBS that gets billed every month, so it wants a decision rather than a default ([er-dev-namespace values.yaml](../infra/k8s/charts/er-dev-namespace/values.yaml) carries the arithmetic).

**`count/jobs.batch`.** At one Job per task attempt, finished Jobs linger until the tombstone-then-TTL cleanup; the 2.3 incident filled a quota of 4 with TTL 3600 and stranded a claim ([plan-to-aws.md](plan-to-aws.md) 2.3). The chart's own batch objects can hold up to seven Jobs at once (`cronjob-tenant-drift` keeps one successful and three failed plus one active; the two upgrade hook Jobs linger while they run; in dev the namespace chart's `bootstrap` hook adds one more, lingering when it fails), so size it as `count/jobs.batch ≥ 7 (+1 in dev) + cap × (1 + ceil(ttl / mean_segment_seconds))`: with cap 3, TTL 300 s and segments of at least 60 s that is 8 + 3×6 = 26, and the dev value is 30 for margin ([er-dev-namespace values.yaml](../infra/k8s/charts/er-dev-namespace/values.yaml)); it is recomputed whenever the cap rises, and the capacity wait is the backstop, never the design.

### 7.3 Kueue: not now, revisit trigger recorded

Kueue's value is fair share across tenants under contention, cohort borrowing and gang admission. It would cost a second queue with its own state (`Workload` objects) and its own priority opinions, and it cannot express one active job per org without one `ClusterQueue` per tenant — quotas and fair sharing are properties of ClusterQueues in a cohort; `LocalQueue`s are namespaced pointers carrying no quota, so "a LocalQueue per org" yields no per-tenant limit. Hundreds of cluster-scoped objects for a rule one index enforces. With a single dispatcher, an 8-vCPU account and tens of tenants, the in-flight cap, `PriorityClass` and `ResourceQuota` cover the need. **Revisit** when the vCPU quotas are raised, more than one class runs concurrently, and fair share *between* tenants becomes a product requirement; adoption then is `suspend: true` at create plus `kueue.x-k8s.io/queue-name`, with no data-model change.

### 7.4 D-ARGO reaffirmed, context amended

D-ARGO's "revisit when a run ever becomes a genuine multi-pod DAG" has fired. The answer is still no, for its own three reasons: a second retry policy ignorant of the exit-code taxonomy, a second source of truth, and a second scheduler that cannot honour `jobs_one_active_per_org` are all still wrong — and the DAG in Postgres with a compiled graph is precisely the second source of truth D-ARGO refused to let Argo be. What we need from a DAG engine is a readiness evaluator over at most eight nodes. Phase 4 amends the decision's context in [infrastructure-decisions.md](infrastructure-decisions.md).

## 8. Cost and latency

Measured ([measurements/runner-startup-20261024.json](measurements/runner-startup-20261024.json)): NodeClaim → Ready 14–18 s, pull 1.6–3.4 s, first session 0.9–1.4 s, submit → container running ~30 s cold; on a warm node inside the 10-minute linger the next pod scheduled → started in ~1 s with no pull; lake attach plus the reconcile probe ~0.8–0.9 s (the record's second-probe elapsed). So a task on a warm node costs **≈3 s of fixed overhead** (pod start, process, attach, work-order fetch) and the first task of a cold run adds ~30 s — the same cold start one pod per run pays today.

| Tenant | Today (one pod per run) | Per task, no fusion | Per task, class-boundary fusion (the default) |
|---|---|---|---|
| S (≤100k) | 30 s cold + run | 30 s + 6×3 s ≈ +18 s, the largest relative tax because S stages are short | one segment: identical to today, with per-stage retry gained |
| M (≤1M) | 30 s cold on runner-sm | +18 s, all on one warm node | one segment: identical to today |
| L (≤10M) | 30 s cold on runner-l for the whole run; the entire run sized for `match` | `match` on runner-l, the rest on runner-sm: two cold starts ≈ +60 s; two node classes concurrently (impossible under the 8-vCPU quota today) | three pods, two node classes: `[init, ingest, standardize]`@M, `[match]`@L, `[reconcile, assemble, finish]`@M; the L node is provisioned for the quadratic stage only and released to `WhenEmpty` within ten minutes of `match` finishing |

The trade with per-task retry: fusion makes a mid-segment failure retry from the failed task, not the segment start (§3.7), so the latency win costs nothing in retry granularity. What it costs is only that a segment runs at its members' shared class, which is by construction the class they would have had anyway.

What keeps DuckDB-on-Kubernetes cheap is unchanged and now reaches every task: spot-first NodePools with on-demand fallback, scale to zero after the 10-minute linger (≈$0.02 per run idle at the measured c6id spot rate including the Auto Mode fee, `runner-sm.yaml`), NVMe spill instead of RAM (D-SIZING: 2 → 6 threads cut a 10M reload 40% with peak memory flat), right-sized requests so two S tasks of different tenants pack onto one 8-vCPU node, and no always-on worker fleet. The standing cost the design adds is one small dispatcher pod. The right-sized `ER_DUCKDB_THREADS`/`MEMORY_LIMIT` finally reach the session — gated by §9.3.

## 9. Determinism, idempotency and resume

### 9.1 Per-stage idempotency today, and the fix

| Stage | Write pattern | Re-executable under the same `run_id` today? | Change |
|---|---|---|---|
| `init` (run row) | `INSERT … status 'running'` after `create_ddl_table` | **No**: DuckLake enforces no keys, a second row appears | `run/delete/by_id.sql` then `start.sql` |
| `ingest` | append to `raw_records`, `ingest_batches`; receipt keyed `{run_id}:{source}`; the batch id is the run id | **No**: duplicate rows (downstream unaffected because standardize keeps the latest row per key; the ledgers grow) | `raw_record/delete/by_batch.sql` + `ingest_batch/delete/by_run.sql` pre-image the whole run, then every delivery is re-landed; no per-source skip, because nothing in `raw_records` keys a source's rows to a delivery (§4.4) |
| `standardize` | `DROP TABLE IF EXISTS` + rebuild | Yes | — |
| `match` | `retire_other_models` then `MERGE … ON (pair, model_version, tf_snapshot_id)`; `MERGE … ON record_key` | Yes | — |
| `reconcile` | `MERGE` membership, `DELETE … absent_from_corpus`, `MERGE` entities, **`INSERT` entity_events** | Membership and entities yes; **`entity_events` no** — a re-run doubles the events the webhook aggregate counts | `entity_event/delete/by_run.sql` before the append |
| `assemble` | golden drop + CTAS; reviews `MERGE`; metrics/quality upsert (`metric_snapshot` natural key `run_id`) | Yes | — |
| `finish` | `UPDATE runs SET status` | Updates every duplicate row today | one row after the `init` fix; `fail.sql` added |
| `train` | writes `model.json` + `tf.csv`; the dispatcher **appends** the TF rows to the shared `ER_TF_PATH` CSV (`append_tf_snapshot`) | **No**: the same snapshot is appended twice | `train` writes `models/<run_id>/{model.json,tf.csv}` on EFS; `register_model` points the draft `trained-{run_id}` registry row at that path; `match` reads the TF file named by the plan's model identity; the shared-CSV append is deleted |
| `maintain` | `flush_inlined_data`, `merge_adjacent_files`, `expire_snapshots` | Yes | — |

### 9.2 The task contract

(1) Idempotent under the same `(run_id, task_id)` — pre-image deletes for append-only relations, drop/recreate or MERGE for the rest, and the `run_stages` row as the last dedicated commit, so "a `succeeded` ledger row in the try's retry lineage ⇒ the output exists" and the probe in §6.3 step 4 is sound. (2) Every input is read `AT (VERSION => snapshot_end)` of the upstream `LakeRef` from the work order where the engine's declared statement allows a pin, and the lake head is checked at task start: the task refuses with `precondition` unless the head equals the pinned upstream `snapshot_end` or no intervening snapshot touched an input relation. This gives per-try determinism under one writer per tenant without re-pinning every declared engine read (reconcile's live `absent_from_corpus` and the live-pinned drift guard keep their `pin: live`). (3) The task records `snapshot_start`/`snapshot_end` on both ledgers after its last commit. (4) Decisions are made once: the mode decision and the drift-guard comparison (`check_mode_preconditions`, today at `dispatcher.rs` inside `run_job`) happen in `init` and travel in `RunPlan`; a task never re-reads the live fingerprint, so a clear hours later cannot flip the mode under a half-finished run.

**Rejected alternative.** One DuckLake transaction per task with the `run_stages` row inside it (k8s-native design). The engine's write discipline is cross-catalog reads into the native temp catalog, then write-backs in **dedicated single-catalog transactions** — "mixing a lake write and cross-catalog reads in one transaction is what it avoids" ([ducklake-lakehouse skill](../.claude/skills/ducklake-lakehouse/SKILL.md), "The shape of every run") — and `write_to_lake_with` itself emits `BEGIN; CREATE OR REPLACE TABLE … ; COMMIT;` ([dedupe-core/src/session/mod.rs](../rust/crates/dedupe-core/src/session/mod.rs)), so an outer transaction would nest. Pre-image deletes plus the ledger row as the last commit are the mechanism; an audit of whether a stage can be made one transaction is a later item, not a Phase 1 rule.

### 9.3 Threads versus the determinism contract

`SessionOptions::deterministic()` pins `threads = 1` so "floating-point aggregation order [stays] stable so results match the golden baselines byte-for-byte" ([dedupe-core/src/session/mod.rs](../rust/crates/dedupe-core/src/session/mod.rs)). Raising `ER_DUCKDB_THREADS` per class is therefore not a pure throughput fix; it touches the determinism contract. **Phase 1 acceptance**: the golden fixtures and the `match_scores` expectations run at 1 and 6 threads and every stage's lake output is byte-equal; any stage that is not order-stable gets an explicit `ORDER BY` or stays at one thread by policy (`NodePolicy` gains `threads: Option<u32>` for that case). No class sets threads above 1 before this passes.

### 9.4 Fingerprint refusal

`clear`, `resume` and adoption compare the binary's structural fingerprint for `jobs.kind` with `jobs.structural_fingerprint` and refuse with the per-node diff; `exec-task` repeats the check with `--expect-structural` so a stale image cannot run a task for a graph it does not have. A policy-hash difference is logged per node and proceeds. `Task::VERSION` is pinned by a test to a hash of the task's declared statement set (the `.sql` files its body calls), closing dagr's fingerprint blindness to task bodies for the common case.

### 9.5 Drift guard, snapshots, resume

DuckLake snapshots are dagr's durable references: the reference is `(relation set, snapshot_end)`, the content hash is the snapshot id itself (immutable), and the existence probe is a pinned `count(*)`. A clear that would pin a cleared task to an upstream `snapshot_end` that `lake_maintain`'s `expire_snapshots` (default `expire_older_than` 7 days) has removed resolves demand upward: the upstream joins the must-run set, and the dialog shows the expanded set. At clear the handler also requires the lake head to equal the pinned `snapshot_end` of the run's last succeeded task unless no intervening snapshot touched an input relation, refusing with 409 naming the snapshot; `force: true` on the request overrides it and is written to `audit_log` with the snapshot it overrode — no column. `match` asserts the plan's fingerprint equals the live one inside its own transaction (writer-only read, `pin: live`) and fails `precondition` otherwise. INV-EQ (incremental scoring equals full) is untouched: the `segment` parameter and the ensemble rule flow through `RunPlan` exactly as they flow through `run_all_ensemble`'s locals today; reconcile and golden remain whole-corpus. Run-all never trains: `match` consumes the registry's active model identity pinned in `RunPlan`; `train` is its own DAG.

## 10. Operability

**UI** (grayscale; state by weight, scale, spacing and motion, never hue, per the [frontend-design-system skill](../.claude/skills/frontend-design-system/SKILL.md)). The job page's phantom `StageTrace` ([job-detail.tsx](../frontend/src/components/runs/job-detail.tsx) renders `progress.stages`, which nothing produces) becomes `useJobTasks(org, jobId)` over `GET /v1/orgs/{org}/jobs/{job_id}/tasks` — wrapped `{tasks}`, ledgered in [envelopes.json](../frontend/src/lib/api/envelopes.json) beside its sibling `/jobs`, typed from `schema.d.ts` `components["schemas"]["TaskInstance"]` rather than another hand-written interface. Per task: state, `try/max_tries`, class (and `escalated_from`), duration, pod and node, exit code, `error_class`/`error_detail`, `termination_reason`, snapshot range, and a disclosure of archived tries. **Retry from here** (steward+, `:clear`) shows the downstream closure and any demand-resolved upstream before confirming; the job-level **Resume** keeps its route as "clear every live non-succeeded task"; **Cancel** appears while `running | canceling`. The frontend's pre-existing `retrying` and `canceling` states ([hooks.ts](../frontend/src/lib/query/hooks.ts) `ACTIVE_JOB_STATES`) finally have producers: a job renders `retrying` when `running` with a live task `up_for_retry`. The BFF route carries `requireOrgAccess`; the mock server gains the route; the a11y sweep gains the screen once a fixture job id exists.

**API.** `TaskInstance` registered in `ApiDoc` (count 12 → 13); the tasks list and `:clear` are additive; `openapi.json` and `schema.d.ts` regenerated and committed together; `er-openapi-check` reports no removal. `/v1/internal/*` is not in the OpenAPI document and not reachable through the BFF.

**Webhooks.** Unchanged: one per settled job outcome — `job.succeeded | failed | canceled` literal at the emit site ([webhook.rs](../rust/crates/er-server/src/webhook.rs) `EVENT_CATALOG`), never per task or try; `job.running` stays absent; no `task.*` entries. A subscriber wanting task detail reads the tasks endpoint from the delivery's `detail_path`. Entity aggregates fire at settle, counting events that are no longer doubled.

**Reports.** `REPORTED_KINDS` and the mail cadence are unchanged; the report gains a tasks table and names the failed task and its pod.

**Logs.** stdout/stderr → Fluent Bit → CloudWatch `/er/dev/containers`, 30 days (the log groups in [observability.tf](../infra/terraform/envs/dev/observability.tf); the DaemonSet's toleration of the runner taint in [fluent-bit-values.yaml](../infra/k8s/observability/fluent-bit-values.yaml)); lines prefixed `job={id} task={name} try={n}`; the UI deep-links a pod name to a Logs Insights query. The control plane never tails and never parses logs; the terminal record is the termination message.

**Traces.** `jobs.trace_context` is written at enqueue — it no longer defaults to `None`: the authenticating layer puts `forwarded_traceparent` on the `Principal` and enqueue threads it through, so the remaining `None` in the tree is a test fixture; the readiness pass mints a child span per try into `task_instances.trace_context`; the pod starts its root span from it, one span per stage, and `force_flush`es with a timeout on every exit path. Pipeline spans are always sampled ([infrastructure.md](infrastructure.md) §10).

**Metrics** (labelled by `resource_class`, `task`, `state`, `error_class`, `termination_reason` — never tenant): `er_tasks_live{state,class}`, `er_task_pending_seconds` (the cold-start and quota tax), `er_task_duration_seconds{task,class}`, `er_task_launches_total{reason}`, `er_quota_waits_total`, `er_observer_reconnects_total{cause}`, `er_scheduled_waiting_on_cap`.

## 11. Build plan

Every phase ships green on `make check` (fmt, clippy, doc, `entities-check`, `skills-check`, test); a phase that touches `entities/` runs `er-entity gen` + `verify`, re-emits `openapi.json`/`schema.d.ts`, and commits generated files together; `make check-e2e` and the kind probe where named. **There is no second production executor at any phase**: the legacy in-process arm is the path for kinds not yet on the allowlist, is never selectable for a listed kind, and is deleted in Phase 4; the DAG path has exactly one executor from the day it exists.

**Phase 0 — prerequisites.** Re-file the Standard vCPU quota requests (nothing in Phase 3 is observable otherwise). `cd rust && cargo add kube --no-default-features --features client,rustls-tls,ring,runtime && cargo add k8s-openapi --features v1_36` — kube 4.0.x under the 1.88 floor ([rust/Cargo.toml](../rust/Cargo.toml)), or raise `rust-version` to 1.89 for 4.2 as a recorded dependency decision; never `aws-lc-rs`; `rustls-platform-verifier`'s licence checked against `deny.toml`. Split the dispatcher into its own Deployment with the `er-dispatcher` SA (no RBAC yet). *Accept:* `make check` and `cargo deny check` green with the dependency; chart renders both Deployments; `tests/egress_guard.rs` untouched.

**Phase 1 — the model and the compiled DAGs, no executor.** `entities/task_instance`, `entities/run_stage` (+ S5 registry, golden, `declared_names`), `run/update/fail.sql`, the idempotency statements of §4.4, the `jobs` columns and job statements; `er-dag` (traits, builder, typestates, assembly, both hashes, readiness, resume); the eight DAGs in `er-server/src/dags/`; `JobKind`; `dispatch_coverage.rs` rewritten to scan `dag_for`; `control_tasks_no_engine.rs`; the four `/v1/internal/attempts` routes with the fallback hash token; `TaskInstance` in `ApiDoc`; the tasks endpoint; the legacy arm gains an `AttemptReporter` that plans rows at claim and drives the same statements per stage with a synthetic launch identity (`claim_launch` with `k8s_job_name = 'inproc'`, `start` with `pod_uid = owner_epoch`, `pod_name` NULL) so the UI shows real stages and real tries before any pod exists — a reporter, not an executor. *Accept:* `tests/dag_assembly.rs` (refusals, hashes, readiness table, resume algorithm); `pg_task_instances.rs` (planning in one transaction with the claim → scheduled → claim → start → heartbeat → finish → `archive_try` + `insert_retry_try` with exact backoff deadlines → clear's three arms and `reopen` → `complete_restart` → a two-member segment through `advance_segment` with the one-pod index holding at every statement boundary); the natural-id SQL mirror byte-equal test; the 1-vs-6-thread byte-equal fixture (§9.3); OpenAPI snapshots regenerated; `pg_e2e` still green.

**Phase 2 — the verb, the channel's credential, the executor for two kinds.** `er-server exec-task` (the terminal record on every deliberate exit path, the lineage ledger probe, the fence, `SessionOptions` from env, segments, SIGTERM through `dedupe_core::session::CancelToken` on DuckDB's `interrupt_handle` with `signal-hook`); the projected-token spike — `er-api` reads the cluster JWKS from the in-cluster API server with its own projected API-server-audience token and verifies a pod's `er-control-plane` token against it (fallback: the owner-referenced Secret); `er-k8s` (vendored observer core, classifier, `PodApi`, `KubeApi`); the launcher, the two-stream watch, adoption, cancel through `canceling`, zombie and stuck-pending sweeps; chart: Role (with `events: list`), `er-runner` SA, the projected token volume on `er-api`, Job template ConfigMap, `executor.*` values; Terraform: the `er-runner` Pod Identity association; PriorityClasses beside the NodePools. `lake_maintain` and `lake_migrate` route to pods under the `executor.dagKinds` allowlist. *Accept:* classifier table tests for every row of §6.5, the `FailedCreate` and `interrupted` rows included; observer suite against scripted 410/stall/403/duplicate frames; `pg_run_all_dag.rs` through the child-process double (`make check-e2e`); on kind: `lake_maintain` end to end, dispatcher restart mid-task adopts with no second pod, cancel mid-`maintain` interrupts DuckDB inside the grace period and the row settles `canceled` with the org slot held through `canceling`, `er-api` with automount off verifies a pod token against the in-cluster JWKS at the fixed path, on kind and on `er-dev`, without following `jwks_uri`; `tests/no_second_executor.rs` lands.

**Phase 3 — the pipeline kinds.** `init` as a real task with `RunPlan`; the engine's idempotency changes (pre-image deletes, per-run TF path, `match` reading the registry's TF path); class per task and OOM escalation; `run_all_*`, `correct`, `train`, `apply_corrections`, `apply_approved`, `provision` on the allowlist; frontend task list and Retry button (mock route, BFF route, coverage floor, palette rule). *Accept:* `DEDUPE_E2E_DUCKLAKE` tier proves a cleared `match` yields byte-identical `match_scores`; kill mid-`reconcile` then relaunch reports from the ledger without re-executing; forced OOM escalates S → M through the double; on kind, `run_all_full` as seven pods — seven distinct Job objects, one launch each, a verified pod uid on every row, and `standardize` pinning the plan plus the corpus `ingest`'s own pod landed (`check_kind.sh` phase 5); on a dev namespace with the quota raised: an L run as three Jobs on two node classes; `pg_http.rs` cancel-while-running asserts no second claim succeeds.

**Status of the dev-namespace half, 2026-11-05.** A real `run_all_full` ran on `er-dev`'s `ci` namespace as seven per-attempt Jobs — seven distinct Job objects, seven distinct pod uids, one launch each, settled `succeeded`, and the lake ledgered `full @ v0001` with the same 18 auto-merged pairs the same corpus produces on kind. Recorded in [measurements/dag-pipeline-eks-20261105.json](measurements/dag-pipeline-eks-20261105.json).

Two things that run did **not** establish, stated because the acceptance above reads as one clause:

- **The class boundary is still unmet.** Every stage ran at class M. `base_10` is 26 source rows, so nothing reaches L, and "three Jobs on two node classes" needs a corpus large enough for the `NodePolicy` to choose one. No corpus of that size has been run through the executor.
- **The in-flight cap was never reached**, because a `run_all` chain runs one stage at a time. So there is no quota-wait or pending-age series yet, and §7.3's Kueue revisit still has no contention data — that needs concurrent tenants, not a bigger single run.

One measured finding worth carrying forward: **a stage chain does not pay a cold node per link.** All seven stages landed on one node, because the inter-stage gaps are 3–5 s and the runner pools reclaim at `consolidateAfter: 30s`. [research-runner-startup.md](research-runner-startup.md) §1 expected the opposite ("every link pays the full cold start"). It is a property of the gap rather than of the design, so a slower hand-over or a wait on the cap would still provision afresh — and consolidation does fire promptly once the chain ends, which the same run confirms.

The kind phase and the dev-namespace one are not interchangeable, and it is worth being exact about which claim each supports. Kind proves the *handoff*: seven real pods, each with the control plane verifying its own projected token, each reading what the previous pod's process left in the lake. It cannot prove the *class boundary* — one node, no Karpenter, every class identical in `values-kind.yaml` — so "three Jobs on two node classes" remains a dev-cluster claim and nothing on kind should be read as covering it.

**Phase 4 — delete the legacy arm.** Remove `run_job`'s DAG-kind arms, `ER_DISPATCH_IN_PROCESS`, `executor.dagKinds`, the `AttemptReporter`, `pg::FailureClass::classify`, `jobs/update/fail_or_retry.sql`'s retry branch, `mark.sql`; rewrite `pg_job_retry.rs` against tasks.

An inventory taken before starting it (2026-11-04) corrected this list in four places, and the corrections matter enough to record:

- **`resume.sql`'s `resume_run_id` branch was not dead — Phase 1 revived it, and it is already gone.** The claim above rested on "no statement writes `jobs.run_id`"; `plan.sql` now sets `run_id = id`, so every planned job has one and resuming a failed one wrote a `resume_run_id` key into `params`, which `GET …/jobs/{id}` serves to the tenant and nothing anywhere reads. Dead *data* in a tenant-visible column is worse than a dead branch, so it was deleted on its own rather than waiting for this phase.
- **`images.runner`/`er-pipeline` and the `dispatcher.launcher` block are already deleted** (#373 and before). What is left is prose that will become wrong, not code: `values.yaml`'s dispatcher `resources` and `terminationGracePeriodSeconds: 300` are both justified entirely by "a killed legacy run strands its row", and that justification evaporates here.
- **`dispatch_coverage.rs` is Phase 1 debt that blocks this phase.** Phase 1 said it would be rewritten to scan `dag_for`; it was not, and it still `include_str!`s `dispatcher.rs` to assert each kind has a textual match arm — including a hard self-check on `lake_maintain`. Deleting the arms fails the file's own self-check, so it must be rewritten first.
- **`classify` has a caller in the NEW arm.** `dags/tasks.rs` maps `pg::FailureClass` into `er_errors::ErError`, so collapsing the two taxonomies is real work ahead of the deletion, not a consequence of it. Likewise `pg::mark` has nine non-legacy callers, and `settle.sql` cannot stand in for `mark(job, "running")` — its `WHERE state IN ('running','canceling')` guard is the wrong shape for a `queued → running` transition.

Two parts of this phase are decisions rather than deletions: **what local development does instead of `ER_DISPATCH_IN_PROCESS`** (`frontend/dev/env.sh` sets it, and `make run` depends on something draining the queue), and **where `run_job`'s tail lands** — the `entity_event::counts_by_kind_live` webhook aggregate is not a kind arm and is lost if the function is deleted around it. Docs: amend D-ARGO (context), D-JOBSPEC, D-PROGRESS, D-SIZING, D-TENANCY in [infrastructure-decisions.md](infrastructure-decisions.md) and add D-TASKJOB, D-CHANNEL, D-FUSION, D-KUEUE; amend [infrastructure.md](infrastructure.md) §6 (`pods/log` out, `events: list` in, Job per task attempt, TTL 300, `count/jobs.batch` arithmetic) and [architecture.md](architecture.md) line 82 (the three indexes, not an advisory lock). *Accept:* `make check` with the deletions; `make check-kind` observed failing once (a deliberately broken classifier row) before it becomes a required check.

**Phase 5 — measure.** A `docs/measurements/` record comparing S/M/L wall-clock and node-hours before and after, the quota-wait and pending-age series, and the Kueue revisit decision against observed contention.

API compatibility throughout: every `/v1/orgs/{org}/jobs*` route keeps its shape; `:resume` keeps its path with clear semantics; new fields are additive and nullable.

## 12. Risks and open questions, ranked

1. ~~**The 8-vCPU quotas.**~~ **Resolved.** Both Standard quotas are 64 (On-Demand `L-1216C47A`, Spot `L-34B43A08`), read back from the account; `inFlightCap` is 3 and a real `lake_maintain` on `er-dev` provisioned a runner node and ran. What this does *not* yet cover is the L split itself: the remaining per-namespace limit is `requests.ephemeral-storage`, which bounds concurrent L pods at two (§7.2), and no L-class run has been observed crossing a class boundary on the dev cluster.
2. **The S3 lake is not openable from Rust.** Pods see the lake only through EFS; the executor inherits this and does not fix it. The `er-runner` role is ready for when it lands.
3. **Multi-threaded determinism.** The Phase 1 fixture decides; worst case `match` stays at one thread by policy and the throughput win shrinks.
4. ~~**The projected-token spike.**~~ **Resolved, and the Secret fallback was never needed.** `er-api` reads the cluster JWKS with its own projected token at the fixed path and verifies a pod's `er-control-plane` token offline, on kind (`check_kind.sh` phase 1 is that verification passing — the pod holds no other credential) and on `er-dev` against the real EKS JWKS. A forged JWT-shaped bearer is refused 401 (phase 4). Still unobserved: **key rotation**, since no JWKS rotation has happened under a running `er-api` — `UNKNOWN_KID_FETCH_FLOOR` and `REFRESH_AFTER` are unit-tested but not exercised live, and that is the one part of this risk still open.
5. **Statement pairs in one transaction.** Every archive-and-insert decision is two declared files on one connection (E012 refuses a writable CTE in a write folder); a crash between them rolls both back, and `pg_task_instances.rs` pins each pair.
6. **Scheduler-loop correctness.** Readiness, sweeps, adoption and the observer's four termination classes each need to be observed failing before they are trusted — the same rule the frontend gates follow.
7. **`count/jobs.batch` and TTL** interact as the 2.3 incident showed; the formula in §7.2 is the design, the quota wait the backstop.
8. **kube MSRV and cargo-deny noise** (4.2 wants 1.89; `multiple-versions` warnings from hyper/tower overlap).
9. **Engine changes for idempotency** touch `er-cli` paths under golden fixtures; INV-EQ gates must stay green.
10. **Fencing is narrow.** A pod on a partitioned node that commits between its last good heartbeat and the fence check is a second writer for one commit; the replacement appears only after the zombie sweep and the ledger probe prevents double execution, but the window exists.
11. **Open:** when `lake_maintain` expires a snapshot an old failed run references, the right UX for "this run can no longer be partially resumed" (today: the dialog shows the expanded must-run set). **Open:** per-tenant `er-runner` ServiceAccounts once the credential chain lands. **Open:** whether `any_failed` ever gets a consumer (a notify task) or is retired from the closed set.

## 13. Non-goals

Dynamic task mapping or any runtime change of the graph's shape (a task that discovers N deliveries iterates). XCom-style values between pods (every edge is a `LakeRef` or a `Plan`; only references travel). A DSL or YAML graph (the compiler is the validator). A local or in-process executor, an `ER_EXECUTOR` flag, or any production path other than `er-server exec-task` in a Kubernetes Job. Argo Workflows (D-ARGO), Celery or Redis (at-least-once redelivery is wrong for an engine that never auto-retries, [backend-design.md](backend-design.md) §5), Kueue for now (§7.3). Pools, `priority_weight` rules, HA schedulers, deferrable tasks, sensors, backfills, timetables and `logical_date` — a run is tenant × kind × trigger and `not_before` is enough. dagr's event-stream fold, libSQL index, blob store and admission pools — each exists because many nodes share one process or because dagr has no database; here the process is the pod and the database is Postgres. Cross-pod sharding of `match` — forbidden by one writer per tenant; the quadratic stage scales vertically through class L, which is why class is per task. Pod log streaming into the control plane. Namespace per tenant (D-TENANCY stands; the forward seam is the `er-runner` ServiceAccount parameter, now real).
