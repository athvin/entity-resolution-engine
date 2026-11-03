//! The pod task types: the typed shapes `dag_for`'s wiring is checked against,
//! and the bodies the executor runs.
//!
//! **One implementation per stage.** Each pipeline body here calls the SAME
//! `er_cli::stage_*` function the composed `run_all_ensemble_observed` entry
//! calls, over a [`RunContext`](crate::runctx::RunContext) resolved from the
//! work order — so a run cannot behave differently depending on which
//! executor claimed it. The declared `Input`/`Output` types are what makes a
//! mis-wired edge a compile error; the bodies are what makes the DAG real.
//!
//! What a body may NOT do: read the control plane. The pod holds no database
//! credential (§6.4), so everything it needs — the tenant env, the delivery
//! root, the resolved model registry — arrives in the order.

use er_dag::{LakeRef, NodePolicy, Outcome, Plan, Task, TaskCtx, TaskError};

use super::edges::{Assembled, Labeled, Landed, Profiled, Resolved, Scored, Standardized};
use super::plans::{
    DrainedActions, FinishReport, LakeInitialized, MaintenanceReport, RunPlan, SchemaReconciled,
    TrainedModel,
};
use super::{JobKind, Session};

fn not_yet_runnable(task: &str) -> TaskError {
    TaskError::Failed(er_errors::ErError::precondition(format!(
        "task body '{task}' is declared but not yet runnable: it lands with the remaining \
         Phase 3 kinds of batch-orchestration-design.md"
    )))
}

/// Resolve this run's inputs from the order AND prepare this pod's session —
/// the first thing every pipeline body does.
///
/// `wants_deliveries` is false for every stage but `ingest`: re-reading the
/// drop directory in `match` would make a stage fail because a delivery was
/// cleaned up after it landed, which is the bug `skip_ingest` was added for.
///
/// **The lexicon is seeded here, not only in `init`.** `er_cli::seed_lexicon`
/// writes a SESSION-LOCAL table (its own doc comment says so), and under the
/// executor every stage is its own process with its own session — so the table
/// `init` seeded died with `init`'s pod and `standardize` failed with
/// "Table with name nickname_variants does not exist". Seeding during
/// resolution rather than per body is deliberate: a body cannot forget to do
/// it, because it cannot run without a context. The seed is re-seedable and
/// derived from the file the order points at, so every pod builds the same
/// table.
fn context(
    ctx: &TaskCtx<'_, Session>,
    wants_deliveries: bool,
) -> Result<crate::runctx::RunContext, TaskError> {
    let models = crate::runctx::models_from_order(&ctx.order.models);
    let run = crate::runctx::RunContext::resolve(&crate::runctx::ResolveRequest {
        lake_env: &ctx.order.lake_env,
        params: &ctx.order.params,
        models: &models,
        drop_root: ctx.order.drop_root.as_deref(),
        wants_deliveries,
    })
    .map_err(|e| {
        // Every resolution refusal is terminal: the same job against the same
        // tenant state would refuse again, so this is `config`, never a retry.
        TaskError::Failed(er_errors::ErError::config(crate::errfmt::chain(&e)))
    })?;
    er_cli::seed_lexicon(ctx.session, &run.lexicon)
        .map_err(|e| engine_error(&format!("seeding the lexicon: {e:#}")))?;
    Ok(run)
}

/// The engine inputs for one stage, rebuilt from the plan `init` published.
///
/// Every stage after `init` takes its mode, clock and rebuild reason from the
/// PLAN rather than deciding again: the drift guard ran once, in `init`, and a
/// second verdict mid-run could disagree with the ledger row already written
/// (§9.5). That is why `RunPlan` carries them at all.
fn inputs_from_plan<'a>(
    ctx: &'a TaskCtx<'_, Session>,
    run: &'a crate::runctx::RunContext,
    model: &'a er_cli::MatchModel<'a>,
    plan: &'a RunPlan,
) -> er_cli::PipelineInputs<'a> {
    er_cli::PipelineInputs {
        lexicon: &run.lexicon,
        deliveries: &run.deliveries,
        model,
        run_id: &ctx.order.run_id,
        ingested_at: &plan.run_at,
        scored_at: &plan.run_at,
        run_at: &plan.run_at,
        mode: &plan.mode,
        rebuild_reason: plan.rebuild_reason.as_deref(),
        segment: run.segment_ref(),
    }
}

/// The lake head after a stage's last commit, as a `LakeRef`'s snapshot.
///
/// A head that cannot be read is NOT an error: the stage committed, and the
/// reference's purpose is to prove existence to the next stage, which probes
/// the relation rather than the number. `-1` is the documented "committed, head
/// unknown" marker — never a real snapshot, so it cannot be mistaken for one.
fn head_or_unknown(session: &Session) -> i64 {
    session.latest_snapshot_version().unwrap_or(-1)
}

macro_rules! declare_task {
    ($(#[$doc:meta])* $ty:ident, $name:literal, $input:ty, $output:ty, writes: $writes:literal $(, policy: $policy:expr)?) => {
        $(#[$doc])*
        #[derive(Default)]
        pub struct $ty;

        impl Task<Session> for $ty {
            const NAME: &'static str = $name;
            const VERSION: u32 = 1;
            type Input = $input;
            type Output = $output;
            const WRITES_LAKE: bool = $writes;

            $(fn policy() -> NodePolicy { $policy })?

            fn run(
                &mut self,
                _ctx: &mut TaskCtx<'_, Session>,
                _input: Self::Input,
            ) -> Result<Outcome<Self::Output>, TaskError> {
                Err(not_yet_runnable($name))
            }
        }
    };
}

/// Opens the lake, makes the drift-guard mode decision ONCE, writes the `runs`
/// row, seeds the lexicon, and emits the `RunPlan` every downstream task
/// consumes (§3.5).
///
/// The mode decision lives here and nowhere else. An incremental run may only
/// extend a corpus scored under the same config / model / standardize /
/// survivorship / lexicon fingerprint; when any of those moved, the run is
/// PROMOTED to full rather than refused — a refusal is just a failed job,
/// while a promotion is what the operator actually wants — and the plan
/// records why, so the ledger and every later stage agree on one answer.
#[derive(Default)]
pub struct Init;

impl Task<Session> for Init {
    const NAME: &'static str = "init";
    const VERSION: u32 = 1;
    type Input = ();
    type Output = Plan<RunPlan>;
    const WRITES_LAKE: bool = true;

    fn run(
        &mut self,
        ctx: &mut TaskCtx<'_, Session>,
        (): Self::Input,
    ) -> Result<Outcome<Self::Output>, TaskError> {
        let run = context(ctx, false)?;
        let model = run.model();
        let members = run.ensemble();

        // A CORRECTION pass has a different init: it pins the PRIOR run's
        // model identity rather than resolving the registry and asking the
        // drift guard, because it re-clusters yesterday's scores and must not
        // re-cluster them under a threshold that was never applied to them
        // (E5b-2c). Same task type, same DAG position, different stage
        // function — which is why the order carries the kind.
        if ctx.order.kind == JobKind::Correct.as_str() {
            let run_at = crate::dispatcher::fmt_epoch(crate::dispatcher::unix_now());
            let snapshot_start = ctx.session.latest_snapshot_version().ok();
            let (model_version, tf_snapshot_id) =
                er_cli::stage_correct_init(ctx.session, &run.config, &ctx.order.run_id, &run_at)
                    .map_err(|e| engine_error(&format!("{e:#}")))?;
            return Ok(Outcome::Done(
                Plan(RunPlan {
                    mode: er_versions::MODE_CORRECTION_PASS.to_string(),
                    rebuild_reason: Some(er_versions::REBUILD_CORRECTION_PASS.to_string()),
                    model_version: Some(model_version),
                    tf_snapshot_id: Some(tf_snapshot_id),
                    lexicon_hash: None,
                    snapshot_start,
                    run_at,
                    corpus_records: None,
                }),
                er_dag::Counters::default(),
            ));
        }

        // The §9.5 baseline: the head BEFORE this run writes anything, which
        // is what a straddled config publish is detected against.
        let snapshot_start = ctx.session.latest_snapshot_version().ok();

        let asked = mode_asked(&ctx.order.kind);
        let prior =
            er_cli::prior_fingerprint(ctx.session).map_err(|e| engine_error(&format!("{e:#}")))?;
        let current = er_cli::current_fingerprint(&run.config, model.model_version, &run.lexicon);
        let decision = er_versions::check_mode_preconditions(prior.as_ref(), &current, asked, true)
            .map_err(|e| {
                TaskError::Failed(er_errors::ErError::precondition(format!("mode guard: {e}")))
            })?;
        let rebuild_reason = decision
            .escalated()
            .then(|| er_versions::REBUILD_OPERATOR.to_string());
        let run_at = crate::dispatcher::fmt_epoch(crate::dispatcher::unix_now());

        let inputs = er_cli::PipelineInputs {
            lexicon: &run.lexicon,
            deliveries: &run.deliveries,
            model: &model,
            run_id: &ctx.order.run_id,
            ingested_at: &run_at,
            scored_at: &run_at,
            run_at: &run_at,
            mode: &decision.mode,
            rebuild_reason: rebuild_reason.as_deref(),
            segment: run.segment_ref(),
        };
        let sole = [er_cli::EnsembleMember {
            model: er_cli::MatchModel { ..model },
            name: er_cli::SOLE_MEMBER_NAME,
        }];
        let scoring = if members.is_empty() {
            &sole[..]
        } else {
            &members[..]
        };
        er_cli::stage_init(ctx.session, &run.config, &inputs, scoring)
            .map_err(|e| engine_error(&format!("{e:#}")))?;

        let (model_version, tf_snapshot_id) = run.identity();
        Ok(Outcome::Done(
            Plan(RunPlan {
                mode: decision.mode.clone(),
                rebuild_reason,
                model_version: Some(model_version),
                tf_snapshot_id: Some(tf_snapshot_id),
                lexicon_hash: None,
                snapshot_start,
                run_at,
                corpus_records: None,
            }),
            er_dag::Counters(serde_json::json!({
                "deliveries": run.deliveries.len(),
                "members": scoring.len(),
                "promoted": decision.escalated(),
            })),
        ))
    }
}

/// The mode the job asked for, by kind — the same `job.kind == "…_incremental"`
/// test the in-process arm makes, reading the kind the order carries.
fn mode_asked(kind: &str) -> &'static str {
    if kind.ends_with("_incremental") {
        er_versions::MODE_INCREMENTAL
    } else {
        er_versions::MODE_FULL
    }
}

/// Lands every delivery and records its batch; fan-out over N deliveries is
/// this task looping internally (§3.1), because a single delivery is not
/// independently retryable — the stage is re-run whole.
///
/// This is the one stage that reads the drop root, so it is the one that asks
/// the resolver for deliveries. Under `{"skip_ingest": true}` — every
/// publish-triggered rebuild — the corpus is already landed and the stage is a
/// no-op over zero deliveries, which is a SUCCESS: refusing would fail a
/// rebuild because its deliveries were cleaned up after landing.
#[derive(Default)]
pub struct Ingest;

impl Task<Session> for Ingest {
    const NAME: &'static str = "ingest";
    const VERSION: u32 = 1;
    type Input = (Plan<RunPlan>,);
    type Output = LakeRef<Landed>;
    const WRITES_LAKE: bool = true;

    fn run(
        &mut self,
        ctx: &mut TaskCtx<'_, Session>,
        (plan,): Self::Input,
    ) -> Result<Outcome<Self::Output>, TaskError> {
        let run = context(ctx, true)?;
        let model = run.model();
        let inputs = inputs_from_plan(ctx, &run, &model, &plan.0);
        if run.deliveries.is_empty() {
            // `NoOp` is exit 10 with `outcome = no_op`, which is a SUCCESS the
            // ledger records as "nothing to do" — not a skip of the graph. A
            // rebuild whose corpus is already landed must leave `ingest`
            // satisfied so `standardize` can fire from it.
            return Ok(Outcome::NoOp(LakeRef::new(head_or_unknown(ctx.session), 0)));
        }
        er_cli::stage_ingest(ctx.session, &run.config, &inputs)
            .map_err(|e| engine_error(&format!("{e:#}")))?;
        Ok(Outcome::Done(
            LakeRef::new(head_or_unknown(ctx.session), 0),
            er_dag::Counters(serde_json::json!({ "deliveries": run.deliveries.len() })),
        ))
    }
}

/// Drop-and-rebuild of `int_std_records`.
#[derive(Default)]
pub struct Standardize;

impl Task<Session> for Standardize {
    const NAME: &'static str = "standardize";
    const VERSION: u32 = 1;
    type Input = (Plan<RunPlan>, LakeRef<Landed>);
    type Output = LakeRef<Standardized>;
    const WRITES_LAKE: bool = true;

    fn run(
        &mut self,
        ctx: &mut TaskCtx<'_, Session>,
        (plan, _landed): Self::Input,
    ) -> Result<Outcome<Self::Output>, TaskError> {
        let run = context(ctx, false)?;
        let model = run.model();
        let inputs = inputs_from_plan(ctx, &run, &model, &plan.0);
        er_cli::stage_standardize(ctx.session, &run.config, &inputs)
            .map_err(|e| engine_error(&format!("{e:#}")))?;
        Ok(Outcome::Done(
            LakeRef::new(head_or_unknown(ctx.session), 0),
            er_dag::Counters::default(),
        ))
    }
}

/// The memory and thread consumer: scores the corpus (full, incremental or
/// segment), then screens the deliveries.
///
/// Takes its mode from the PLAN, never from a second drift-guard call: the
/// guard ran in `init`, the ledger row already records its verdict, and a
/// second opinion mid-run could disagree with it (§9.5).
#[derive(Default)]
pub struct Match;

impl Task<Session> for Match {
    const NAME: &'static str = "match";
    const VERSION: u32 = 1;
    type Input = (Plan<RunPlan>, LakeRef<Standardized>);
    type Output = LakeRef<Scored>;
    const WRITES_LAKE: bool = true;

    fn policy() -> NodePolicy {
        NodePolicy {
            escalate_on_oom: true,
            ..NodePolicy::default()
        }
    }

    fn run(
        &mut self,
        ctx: &mut TaskCtx<'_, Session>,
        (plan, _std): Self::Input,
    ) -> Result<Outcome<Self::Output>, TaskError> {
        let run = context(ctx, false)?;
        let model = run.model();
        let members = run.ensemble();
        let inputs = inputs_from_plan(ctx, &run, &model, &plan.0);
        let sole = [er_cli::EnsembleMember {
            model: er_cli::MatchModel { ..model },
            name: er_cli::SOLE_MEMBER_NAME,
        }];
        let scoring = if members.is_empty() {
            &sole[..]
        } else {
            &members[..]
        };
        er_cli::stage_match(ctx.session, &run.config, &inputs, scoring)
            .map_err(|e| engine_error(&format!("{e:#}")))?;
        Ok(Outcome::Done(
            LakeRef::new(head_or_unknown(ctx.session), 0),
            er_dag::Counters(serde_json::json!({
                "mode": plan.0.mode,
                "members": scoring.len(),
                "segment": run.segment.as_ref().map(|(name, _)| name.clone()),
            })),
        ))
    }
}

/// Clusters the above-threshold graph into entities: the membership/entities
/// MERGE plus the run's `entity_events`.
///
/// Whole-corpus in BOTH modes — see `er_cli::stage_reconcile` for why that is
/// a choice rather than an omission.
#[derive(Default)]
pub struct Reconcile;

impl Task<Session> for Reconcile {
    const NAME: &'static str = "reconcile";
    const VERSION: u32 = 1;
    type Input = (Plan<RunPlan>, LakeRef<Scored>);
    type Output = LakeRef<Resolved>;
    const WRITES_LAKE: bool = true;

    fn policy() -> NodePolicy {
        NodePolicy {
            escalate_on_oom: true,
            ..NodePolicy::default()
        }
    }

    fn run(
        &mut self,
        ctx: &mut TaskCtx<'_, Session>,
        (plan, _scored): Self::Input,
    ) -> Result<Outcome<Self::Output>, TaskError> {
        let run = context(ctx, false)?;
        let model = run.model();
        let inputs = inputs_from_plan(ctx, &run, &model, &plan.0);
        er_cli::stage_reconcile(ctx.session, &run.config, &inputs)
            .map_err(|e| engine_error(&format!("{e:#}")))?;
        Ok(Outcome::Done(
            LakeRef::new(head_or_unknown(ctx.session), 0),
            er_dag::Counters::default(),
        ))
    }
}

/// Golden records, the review queue, then metrics and quality (the last two
/// best-effort — see `er_cli::stage_assemble`).
#[derive(Default)]
pub struct Assemble;

impl Task<Session> for Assemble {
    const NAME: &'static str = "assemble";
    const VERSION: u32 = 1;
    type Input = (Plan<RunPlan>, LakeRef<Resolved>);
    type Output = LakeRef<Assembled>;
    const WRITES_LAKE: bool = true;

    fn run(
        &mut self,
        ctx: &mut TaskCtx<'_, Session>,
        (plan, _resolved): Self::Input,
    ) -> Result<Outcome<Self::Output>, TaskError> {
        let run = context(ctx, false)?;
        let model = run.model();
        let inputs = inputs_from_plan(ctx, &run, &model, &plan.0);
        er_cli::stage_assemble(ctx.session, &run.config, &inputs)
            .map_err(|e| engine_error(&format!("{e:#}")))?;
        Ok(Outcome::Done(
            LakeRef::new(head_or_unknown(ctx.session), 0),
            er_dag::Counters::default(),
        ))
    }
}

/// The teardown epilogue: closes the run's ledger row.
///
/// Runs under `canceling` too (§4.3, §5.6) — which is the whole point of a
/// teardown node: a canceled or failed run must still have its `runs` row
/// closed, or the UI shows it running forever. The outcome it records is this
/// task's own view: `er_cli::stage_finish` writes the end stamp, and a run
/// whose body failed was already marked by the dispatcher's settle.
#[derive(Default)]
pub struct Finish;

impl Task<Session> for Finish {
    const NAME: &'static str = "finish";
    const VERSION: u32 = 1;
    type Input = ();
    type Output = Plan<FinishReport>;
    const WRITES_LAKE: bool = true;

    fn policy() -> NodePolicy {
        NodePolicy {
            teardown: true,
            ..NodePolicy::default()
        }
    }

    fn run(
        &mut self,
        ctx: &mut TaskCtx<'_, Session>,
        (): Self::Input,
    ) -> Result<Outcome<Self::Output>, TaskError> {
        // No `RunPlan` upstream: a teardown node fires whatever its upstreams
        // did, including a failed `init` that never published one. The run
        // clock therefore comes from this task rather than from the plan.
        let run_at = crate::dispatcher::fmt_epoch(crate::dispatcher::unix_now());
        let ended = er_cli::stage_finish(
            ctx.session,
            &er_cli::PipelineInputs {
                lexicon: &[],
                deliveries: &[],
                model: &er_cli::MatchModel {
                    settings_json: "{}",
                    tf_csv_path: std::path::Path::new(""),
                    model_version: "",
                    tf_snapshot_id: "",
                },
                run_id: &ctx.order.run_id,
                ingested_at: &run_at,
                scored_at: &run_at,
                run_at: &run_at,
                mode: er_versions::MODE_FULL,
                rebuild_reason: None,
                segment: None,
            },
        );
        // A run row that was never written (a failed `init`) has nothing to
        // close, and saying so must not fail the teardown — the job's own
        // outcome is already settled by then.
        let closed = match ended {
            Ok(()) => true,
            Err(e) => {
                eprintln!("finish: the run row could not be closed: {e:#}");
                false
            }
        };
        Ok(Outcome::Done(
            Plan(FinishReport {
                settled: if closed { "closed" } else { "no_run_row" }.to_string(),
            }),
            er_dag::Counters::default(),
        ))
    }
}

/// `correct`'s reconcile: re-clusters from the lake's OWN scores, pinned to
/// the model identity `init` read from `run/read/latest_live` (§3.5).
///
/// The same clustering pass `run_all` uses — the difference is upstream, not
/// here: nothing re-scored, so the edge set is yesterday's and the identity on
/// the run row is the one that produced it.
#[derive(Default)]
pub struct CorrectReconcile;

impl Task<Session> for CorrectReconcile {
    const NAME: &'static str = "reconcile";
    const VERSION: u32 = 1;
    type Input = (Plan<RunPlan>,);
    type Output = LakeRef<Resolved>;
    const WRITES_LAKE: bool = true;

    fn policy() -> NodePolicy {
        NodePolicy {
            escalate_on_oom: true,
            ..NodePolicy::default()
        }
    }

    fn run(
        &mut self,
        ctx: &mut TaskCtx<'_, Session>,
        (plan,): Self::Input,
    ) -> Result<Outcome<Self::Output>, TaskError> {
        let run = context(ctx, false)?;
        let model = run.model();
        let inputs = inputs_from_plan(ctx, &run, &model, &plan.0);
        er_cli::stage_reconcile(ctx.session, &run.config, &inputs)
            .map_err(|e| engine_error(&format!("{e:#}")))?;
        Ok(Outcome::Done(
            LakeRef::new(head_or_unknown(ctx.session), 0),
            er_dag::Counters::default(),
        ))
    }
}

declare_task!(
    /// `apply_corrections`' init: consumes the drained-action snapshot and
    /// emits the pass's `RunPlan` (§3.5).
    CorrectionsInit, "init", (Plan<DrainedActions>,), Plan<RunPlan>, writes: true
);

declare_task!(
    /// Applies the drained staged actions to the lake (assertions, review
    /// resolutions): pod-side because the payload needs a lake open (§3.5).
    ApplyStaged, "apply_staged", (Plan<RunPlan>,), LakeRef<Labeled>, writes: true
);

declare_task!(
    /// `apply_corrections`' reconcile: consumes the freshly-landed labels.
    CorrectionsReconcile, "reconcile", (Plan<RunPlan>, LakeRef<Labeled>), LakeRef<Resolved>, writes: true,
    policy: NodePolicy { escalate_on_oom: true, ..NodePolicy::default() }
);

/// The reconcile half of `init` for kinds with no run row (§3.5); for
/// `lake_migrate` the reconcile IS the work. REAL since Phase 2: the first
/// body the executor runs.
#[derive(Default)]
pub struct ReconcileSchema;

impl Task<Session> for ReconcileSchema {
    const NAME: &'static str = "reconcile_schema";
    const VERSION: u32 = 1;
    type Input = ();
    type Output = Plan<SchemaReconciled>;
    const WRITES_LAKE: bool = true;

    fn run(
        &mut self,
        ctx: &mut TaskCtx<'_, Session>,
        (): Self::Input,
    ) -> Result<Outcome<Self::Output>, TaskError> {
        // `reconcile_lake_env` opens its own session from the tenant env — the
        // ctx session stays the probe/ledger handle.
        let outcome = crate::schema::reconcile_lake_env(&ctx.order.lake_env);
        let (status, hash, detail) = crate::schema::tenant_state(&outcome);
        match outcome {
            Ok(_) => Ok(Outcome::Done(
                Plan(SchemaReconciled {
                    status: status.to_string(),
                    catalog_hash: hash,
                    detail,
                }),
                er_dag::Counters::default(),
            )),
            Err(message) => Err(engine_error(&message)),
        }
    }
}

/// Profile one corpus stage and record it. REAL, like `Maintain`: the work is
/// one `er_cli` call with no fan-out, so there is nothing to wait for an
/// executor to supply.
///
/// The subject, tier and sampling come from the job's params rather than from
/// the lake env, because they are per-request rather than per-tenant — asking
/// for the golden records at tier 2 is a different question from asking for the
/// standardized corpus at tier 1, and the same tenant asks both.
#[derive(Default)]
pub struct ProfileCorpus;

impl Task<Session> for ProfileCorpus {
    const NAME: &'static str = "profile";
    const VERSION: u32 = 1;
    type Input = (Plan<SchemaReconciled>,);
    type Output = LakeRef<Profiled>;
    const WRITES_LAKE: bool = true;

    fn run(
        &mut self,
        ctx: &mut TaskCtx<'_, Session>,
        _input: Self::Input,
    ) -> Result<Outcome<Self::Output>, TaskError> {
        let request = crate::profile_params::ProfileRequest::from_params(&ctx.order.params)
            .map_err(|message| TaskError::Failed(er_errors::ErError::precondition(message)))?;
        let account = ctx
            .order
            .lake_env
            .get("ER_ENTITY_TYPE")
            .and_then(serde_json::Value::as_str)
            == Some("account");
        let captured_at = crate::dispatcher::fmt_epoch(crate::dispatcher::unix_now());
        match request.run(ctx.session, account, &captured_at) {
            Ok(report) => Ok(Outcome::Done(
                LakeRef::new(report.snapshot, report.column_count),
                er_dag::Counters(serde_json::json!({
                    "columns_profiled": report.column_count,
                    "rows_read": report.row_count,
                    "findings": report.findings.len(),
                })),
            )),
            Err(message) => Err(engine_error(&message)),
        }
    }
}

/// `flush_inlined_data` + `merge_adjacent_files` + `expire_snapshots`. REAL
/// since Phase 2.
#[derive(Default)]
pub struct Maintain;

impl Task<Session> for Maintain {
    const NAME: &'static str = "maintain";
    const VERSION: u32 = 1;
    type Input = (Plan<SchemaReconciled>,);
    type Output = Plan<MaintenanceReport>;
    const WRITES_LAKE: bool = true;

    fn run(
        &mut self,
        ctx: &mut TaskCtx<'_, Session>,
        _input: Self::Input,
    ) -> Result<Outcome<Self::Output>, TaskError> {
        let expire = ctx
            .order
            .lake_env
            .get("ER_EXPIRE_OLDER_THAN")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("7 days")
            .to_string();
        match er_cli::run_maintain(ctx.session, &expire) {
            Ok(report) => Ok(Outcome::Done(
                Plan(MaintenanceReport { report }),
                er_dag::Counters::default(),
            )),
            Err(e) => Err(engine_error(&format!("{e:#}"))),
        }
    }
}

/// Map an engine failure string onto the er-errors taxonomy the executor's
/// retry policy reads, through the same classifier the legacy arm uses.
fn engine_error(message: &str) -> TaskError {
    let class = crate::pg::FailureClass::classify(message);
    let err = match class {
        crate::pg::FailureClass::TransientIo => {
            er_errors::ErError::stage_failure(message.to_string())
                .with_class(er_errors::ErrorClass::TransientIo)
        }
        crate::pg::FailureClass::Precondition => {
            er_errors::ErError::precondition(message.to_string())
        }
        crate::pg::FailureClass::Config => er_errors::ErError::config(message.to_string()),
        crate::pg::FailureClass::Internal => er_errors::ErError::stage_failure(message.to_string()),
    };
    TaskError::Failed(err)
}

/// Trains and writes `models/<run_id>/{model.json,tf.csv}` on the shared
/// filesystem; no lake write and no run row (§3.5, §9.1).
///
/// The artifacts go to EFS rather than into the plan because the REGISTRY
/// write is the control task's half (`register_model`), and a settings
/// document on the ledger row would put a multi-kilobyte blob in Postgres for
/// no reason. Both halves mount the same filesystem, so a path is enough.
///
/// The per-run directory is the §9.1 idempotency key: a re-run overwrites its
/// own artifacts and nobody else's.
#[derive(Default)]
pub struct Train;

impl Task<Session> for Train {
    const NAME: &'static str = "train";
    const VERSION: u32 = 1;
    type Input = (Plan<SchemaReconciled>,);
    type Output = Plan<TrainedModel>;
    const WRITES_LAKE: bool = false;

    fn run(
        &mut self,
        ctx: &mut TaskCtx<'_, Session>,
        _input: Self::Input,
    ) -> Result<Outcome<Self::Output>, TaskError> {
        let run = context(ctx, false)?;
        let training = run.config.training.clone();
        // One label per training run, derived from the run id so two trains
        // cannot collide and so a registry row traces back to the job.
        let label = format!("trained-{}", ctx.order.run_id);
        let tf_snapshot_id = format!("tf-{}", ctx.order.run_id);

        // The TEMPLATE is the structure to retrain: the active version's, so
        // training refines what is LIVE rather than whatever is on disk. The
        // resolver already picked it (registry first, env file second), which
        // is the same precedence the in-process arm uses.
        let template = run.settings_json.clone();
        let labels = crate::runctx::labels_from_order(&ctx.order.labels);

        let dir = models_dir(ctx, &run)?;
        std::fs::create_dir_all(&dir)
            .map_err(|e| engine_error(&format!("train artifact dir {}: {e}", dir.display())))?;
        let out_model = dir.join("model.json");
        let out_tf = dir.join("tf.csv");
        er_cli::run_train(
            ctx.session,
            &er_cli::TrainInputs {
                template_settings_json: &template,
                deterministic_rules: &training.deterministic_rules,
                em_rules: &training.em_blocking_rules,
                recall: training.recall,
                #[expect(
                    clippy::cast_precision_loss,
                    reason = "a pair budget's magnitude is what matters, and f64 holds every \
                              value an i64 budget can usefully take"
                )]
                u_max_pairs: training.u_max_pairs as f64,
                model_version: &label,
                tf_snapshot_id: &tf_snapshot_id,
                match_labels: &labels,
            },
            &out_model,
            &out_tf,
        )
        .map_err(|e| engine_error(&format!("training: {e:#}")))?;

        // Append this snapshot's rows to the shared frozen-TF CSV: the file is
        // keyed by (model_version, tf_snapshot_id, column), so snapshots
        // coexist and each model reads only its own rows.
        let tf_rows = std::fs::read_to_string(&out_tf)
            .map_err(|e| engine_error(&format!("read trained tf: {e}")))?;
        let shared_tf = run.tf_path.to_string_lossy().to_string();
        crate::dispatcher::append_tf_snapshot(&shared_tf, &tf_rows)
            .map_err(|e| engine_error(&e))?;

        Ok(Outcome::Done(
            Plan(TrainedModel {
                model_version: label,
                tf_snapshot_id,
                model_path: out_model.to_string_lossy().to_string(),
                tf_path: shared_tf,
                entity_type: if run.config.entity_type.is_person() {
                    "person".to_string()
                } else {
                    "account".to_string()
                },
                config_hash: er_config::config_hash(&run.config),
                labels_used: labels.len(),
            }),
            er_dag::Counters(serde_json::json!({ "labels_used": labels.len() })),
        ))
    }
}

/// Where a training pass writes its artifacts: `<models root>/<run_id>/`.
///
/// `ER_MODELS_ROOT` when the tenant env names one, else the directory holding
/// the shared TF file — which is on the same shared filesystem by construction,
/// since that is where the frozen snapshots already live.
fn models_dir(
    ctx: &TaskCtx<'_, Session>,
    run: &crate::runctx::RunContext,
) -> Result<std::path::PathBuf, TaskError> {
    let root = ctx
        .order
        .lake_env
        .get("ER_MODELS_ROOT")
        .and_then(serde_json::Value::as_str)
        .map(std::path::PathBuf::from)
        .or_else(|| run.tf_path.parent().map(|p| p.join("models")))
        .ok_or_else(|| {
            TaskError::Failed(er_errors::ErError::config(
                "no ER_MODELS_ROOT and ER_TF_PATH has no parent directory, so a trained \
                 model has nowhere to land"
                    .to_string(),
            ))
        })?;
    Ok(root.join(&ctx.order.run_id))
}

declare_task!(
    /// Creates and reconciles the new tenant's lake (today's
    /// `reconcile_lake_env` half of `provision_tenant`): anything opening a
    /// lake is a pod task (§3.5).
    LakeInit, "lake_init", (Plan<super::plans::CatalogCreated>,), Plan<LakeInitialized>, writes: true
);
