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
//! root, the resolved model registry, the training labels, the drained staged
//! actions — arrives in the order.
//!
//! Every body here is real. The `declare_task!` stub macro that stood in for
//! them through Phases 1 and 2 is gone, which is the honest marker that the
//! pod half of the design is built: there is no longer a task the executor
//! declares but cannot run.

use er_dag::{LakeRef, NodePolicy, Outcome, Plan, Task, TaskCtx, TaskError};

use super::edges::{Assembled, Landed, Profiled, Resolved, Scored, Standardized};
use super::plans::{
    DrainedActions, ExtractReport, FinishReport, LakeInitialized, MaintenanceReport, RunPlan,
    SchemaReconciled, TrainedModel,
};
use super::{JobKind, Session};

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
    er_cli::seed_ignore_words(ctx.session, &run.ignore_words)
        .map_err(|e| engine_error(&format!("seeding the ignore-words dictionary: {e:#}")))?;
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
        ignore_words: &run.ignore_words,
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
        let current = er_cli::current_fingerprint(
            &run.config,
            model.model_version,
            &run.lexicon,
            &run.ignore_words,
        );
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
            ignore_words: &run.ignore_words,
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
                ignore_words: &[],
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
                tracing::warn!(error = %format!("{e:#}"), "the run row could not be closed");
                false
            }
        };
        // The entity-event counts for the webhook aggregate (O3c-2), read here
        // because this is the last task of the run AND the last place a lake
        // session exists. The control plane settles the job moments later and
        // can emit, but cannot count — so the number has to cross in this
        // task's output. See `FinishReport`.
        //
        // Best-effort, exactly as the legacy arm was: the run's results are
        // already written, so failing a teardown because a COUNT could not be
        // read would destroy work in order to describe it. An empty vector
        // means "nothing to announce", which is also the right answer for a
        // kind of job that reconciles nothing.
        let entity_counts = match er_entities::entity_event::read::counts_by_kind_live(
            ctx.session.connection(),
            &ctx.order.run_id,
        ) {
            Ok(rows) => rows
                .into_iter()
                .filter_map(|r| {
                    let n = r.n.unwrap_or(0);
                    (n > 0).then_some(crate::dags::plans::EntityCount {
                        event_type: r.event_type,
                        n,
                    })
                })
                .collect(),
            Err(e) => {
                tracing::warn!(
                    er.run_id = %ctx.order.run_id,
                    error = %e,
                    "the entity-event counts are unavailable"
                );
                Vec::new()
            }
        };
        Ok(Outcome::Done(
            Plan(FinishReport {
                settled: if closed { "closed" } else { "no_run_row" }.to_string(),
                entity_counts,
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

/// `apply_corrections`' init: consumes the drained-action snapshot and emits
/// the pass's `RunPlan` (§3.5).
///
/// A correction pass, like `correct`: it pins the PRIOR run's model identity
/// rather than resolving the registry, because it re-clusters scores that
/// identity produced.
#[derive(Default)]
pub struct CorrectionsInit;

impl Task<Session> for CorrectionsInit {
    const NAME: &'static str = "init";
    const VERSION: u32 = 1;
    type Input = (Plan<DrainedActions>,);
    type Output = Plan<RunPlan>;
    const WRITES_LAKE: bool = true;

    fn run(
        &mut self,
        ctx: &mut TaskCtx<'_, Session>,
        (drained,): Self::Input,
    ) -> Result<Outcome<Self::Output>, TaskError> {
        let run = context(ctx, false)?;
        let run_at = crate::dispatcher::fmt_epoch(crate::dispatcher::unix_now());
        let snapshot_start = ctx.session.latest_snapshot_version().ok();
        let (model_version, tf_snapshot_id) =
            er_cli::stage_correct_init(ctx.session, &run.config, &ctx.order.run_id, &run_at)
                .map_err(|e| engine_error(&format!("{e:#}")))?;
        Ok(Outcome::Done(
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
            er_dag::Counters(serde_json::json!({ "drained": drained.0.action_ids.len() })),
        ))
    }
}

/// Applies the drained staged actions to the lake (assertions, review
/// resolutions): pod-side because the payload needs a lake open (§3.5).
///
/// Its output is a PLAN rather than a lake reference, which is a deliberate
/// departure from the design sketch: applying derives pair labels and review
/// resolutions that only Postgres can store, and a pod cannot store them
/// (§6.4). The ledger is the only channel between the halves, so the receipt
/// carries them to `settle_actions`.
///
/// Applying stops at the FIRST failing action: it and the rest stay pending
/// for a later pass rather than being skipped, and the ids that did land are
/// still reported so they are not applied twice.
#[derive(Default)]
pub struct ApplyStaged;

impl Task<Session> for ApplyStaged {
    const NAME: &'static str = "apply_staged";
    const VERSION: u32 = 1;
    type Input = (Plan<RunPlan>, Plan<super::plans::DrainedActions>);
    type Output = Plan<super::plans::StagedApplied>;
    const WRITES_LAKE: bool = true;

    fn run(
        &mut self,
        ctx: &mut TaskCtx<'_, Session>,
        (plan, drained): Self::Input,
    ) -> Result<Outcome<Self::Output>, TaskError> {
        // The actions arrive on `drain_actions`'s edge. A pod holds no
        // control-plane credential (§6.4), so it cannot read
        // `staged_steward_actions` itself and whatever it applies has to be
        // handed to it.
        //
        // This used to read `order.params["staged_actions"]`, a key nothing in
        // the tree ever wrote — so every correction pass applied nothing and
        // settled `succeeded` (#760). A typed pin cannot fail that way: the
        // edge is declared in `Input`, the DAG wires it, and
        // `dag_assembly.rs` checks the wiring.
        let pending: Vec<StagedAction> = drained
            .0
            .actions
            .into_iter()
            .map(|a| StagedAction {
                id: a.id,
                action: a.action,
                created_by: Some(a.created_by),
            })
            .collect();
        let now = &plan.0.run_at;
        let mut applied = Vec::new();
        let mut labels = Vec::new();
        let mut resolutions = Vec::new();
        for staged in &pending {
            match er_cli::apply_staged_action_labeled(ctx.session, &staged.id, &staged.action, now)
            {
                Ok(derived) => {
                    labels.extend(derived.into_iter().map(|l| super::plans::DerivedPairLabel {
                        rec_a_key: l.rec_a_key,
                        rec_b_key: l.rec_b_key,
                        verdict: l.verdict.to_string(),
                        decided_by: staged.created_by.clone(),
                    }));
                }
                Err(e) => {
                    // The prefix that landed is still the receipt: failing
                    // without it would re-apply those actions on the next pass.
                    return Err(engine_error(&format!("apply {}: {e:#}", staged.id)));
                }
            }
            // Recorded after the apply succeeded, so the event cannot claim a
            // resolution the lake never took.
            if staged
                .action
                .get("type")
                .and_then(serde_json::Value::as_str)
                == Some("resolve_review")
            {
                let field = |k: &str| {
                    staged
                        .action
                        .get(k)
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                };
                resolutions.push((field("review_id"), field("resolution")));
            }
            applied.push(staged.id.clone());
        }
        let count = applied.len();
        Ok(Outcome::Done(
            Plan(super::plans::StagedApplied {
                applied,
                labels,
                resolutions,
                snapshot_end: ctx.session.latest_snapshot_version().ok(),
            }),
            er_dag::Counters(serde_json::json!({ "applied": count })),
        ))
    }
}

/// One staged action as the work order carries it.
struct StagedAction {
    id: String,
    action: serde_json::Value,
    created_by: Option<String>,
}

/// `apply_corrections`' reconcile: consumes the freshly-applied actions.
///
/// The same clustering pass as everywhere else — what differs is that the
/// assertions it clusters under were written moments ago by `apply_staged`.
#[derive(Default)]
pub struct CorrectionsReconcile;

impl Task<Session> for CorrectionsReconcile {
    const NAME: &'static str = "reconcile";
    const VERSION: u32 = 1;
    type Input = (Plan<RunPlan>, Plan<super::plans::StagedApplied>);
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
        (plan, _applied): Self::Input,
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

/// The reconcile half of `init` for kinds with no run row (§3.5); for
/// `lake_migrate` the reconcile IS the work. REAL since Phase 2: the first
/// body the executor runs.
/// Scan the CORRECTED corpus for what a correction pass changed, so the
/// control half can queue one `entity.write_back` per entity (O4).
///
/// Driven off the derived LABELS rather than the raw resolutions, which gets
/// dismissals right for free: a dismissal yields no label because it is not a
/// verdict about the pair, and it changes no master either, so there is
/// nothing to write back. The labels also carry the canonical pair keys, which
/// is what the lookup needs.
///
/// Both of a pair's keys are resolved and the entity ids deduplicated: a
/// `match` collapses them to one entity and a `no_match` leaves two, and a
/// subscriber wanting to apply the change physically needs whichever it is
/// without having to work it out.
///
/// **Best-effort in the body, deliberately.** The job's outcome comes from its
/// non-teardown leaves, so a task that FAILED here would fail a pass whose
/// corrections are already applied and marked — reporting on work would
/// destroy it. Every skip is logged with its reason, because a write-back that
/// silently does not happen is indistinguishable from a tenant whose endpoint
/// is fine. That is the same bargain the legacy arm struck; it is expressed
/// here by always returning `Done`.
///
/// It does NOT check for subscribers first. The legacy half did, to avoid
/// paying for a lake session nobody wanted, but this task's session is already
/// open — being a pod task is what buys the lake read — and the subscriber
/// list is Postgres, which a pod cannot reach (§6.4). `emit_write_back` skips
/// instead, so the cost of a tenant with no subscriber is this scan rather
/// than a session.
#[derive(Default)]
pub struct WriteBackScan;

impl Task<Session> for WriteBackScan {
    const NAME: &'static str = "write_back_scan";
    const VERSION: u32 = 1;
    type Input = (
        Plan<RunPlan>,
        Plan<super::plans::StagedApplied>,
        LakeRef<Assembled>,
    );
    type Output = Plan<super::plans::WriteBackScanned>;
    const WRITES_LAKE: bool = false;

    fn run(
        &mut self,
        ctx: &mut TaskCtx<'_, Session>,
        (_plan, applied, _assembled): Self::Input,
    ) -> Result<Outcome<Self::Output>, TaskError> {
        let labels = &applied.0.labels;
        if labels.is_empty() {
            return Ok(Outcome::Done(
                Plan(super::plans::WriteBackScanned::default()),
                er_dag::Counters(serde_json::json!({ "write_back": 0 })),
            ));
        }
        let entity_type = crate::entity_type_from_env(&ctx.order.lake_env);
        let snapshot = match crate::read::latest_snapshot(ctx.session) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(er.run_id = %ctx.order.run_id, error = %e, "the write-back scan was skipped: no snapshot");
                return Ok(Outcome::Done(
                    Plan(super::plans::WriteBackScanned::default()),
                    er_dag::Counters(serde_json::json!({ "write_back": 0 })),
                ));
            }
        };
        let mut entity_ids: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        for label in labels {
            for key in [&label.rec_a_key, &label.rec_b_key] {
                match crate::read::entity_of_record_key(ctx.session, snapshot, key) {
                    Ok(Some(id)) => {
                        entity_ids.insert(id);
                    }
                    // A record the corpus no longer has resolves to nothing.
                    // That is a real outcome — it was deleted between staging
                    // and the pass — not an error, and the other half of the
                    // pair may still resolve.
                    Ok(None) => {}
                    Err(e) => {
                        tracing::warn!(er.run_id = %ctx.order.run_id, er.record_key = %key, error = %e, "the write-back lookup failed");
                    }
                }
            }
        }
        let mut entities = Vec::new();
        for entity_id in &entity_ids {
            match crate::read::entity_detail(ctx.session, entity_type, snapshot, entity_id) {
                Ok(Some(detail)) => entities.push(super::plans::WriteBackEntity {
                    entity_id: entity_id.clone(),
                    master: detail
                        .get("golden")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null),
                    members: detail
                        .get("members")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null),
                }),
                Ok(None) => {}
                Err(e) => {
                    tracing::warn!(er.run_id = %ctx.order.run_id, er.entity_id = %entity_id, error = %e, "the write-back detail lookup failed");
                }
            }
        }
        let count = entities.len();
        Ok(Outcome::Done(
            Plan(super::plans::WriteBackScanned { entities }),
            er_dag::Counters(serde_json::json!({ "write_back": count })),
        ))
    }
}

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
        // Reconcile through the session this task ALREADY HOLDS — the same
        // correction #778 made to `lake_init`, which did not go far enough.
        //
        // This opened a SECOND session via `reconcile_lake_env` while the ctx
        // session was attached to the same catalog, and the ctx session — which
        // lives to the end of `exec_task::run_inner`, so it is dropped last —
        // wrote its earlier view back over the committed metadata. The reconcile
        // therefore never persisted, for `lake_migrate`, `lake_maintain`,
        // `train` and `profile` alike, since every one of them opens with this
        // task.
        //
        // What it looked like: the deploy's tenant sweep reported `migrated`
        // with the same 22 relations applied on every single run and never
        // converged to `current`. `pg_migrate_tenants.rs` is what caught it,
        // but only once it ran through the executor instead of the legacy arm —
        // the arm did its reconcile in the dispatcher's own process, one
        // session, so the defect was invisible there.
        let outcome = er_entity_diff::reconcile_lake(
            ctx.session.connection(),
            &er_entities::catalog::catalog(),
            &crate::schema::applied_by(),
        )
        .map_err(|e| match e {
            // The breaking message already lists every difference.
            e @ er_entity_diff::ReconcileError::Breaking(_) => e.to_string(),
            other => format!("lake schema: {}", crate::errfmt::chain(&other)),
        });
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

/// Pull one sObject from Salesforce into a drop-root CSV delivery — the
/// Bulk API 2.0 half of the `extract` kind (E2's remaining gap: "our ingestion
/// reads S3 and CSV, not an org").
///
/// A pod task, not a control one, although nothing here opens the lake: a Bulk
/// API extract blocks for as long as Salesforce takes to run the query, and a
/// control body runs inside the dispatcher, where minutes of waiting would
/// stall every other tenant's launches. The pod pays for one idle node
/// instead; the Postgres half (`advance_sf_cursor`) stays with the dispatcher.
///
/// The delivery lands exactly the way an upload or the S3 poller lands one —
/// staged as `.{source}.{delivery_id}.csv.part`, renamed to `{source}.csv` —
/// so the downstream `run_all_incremental` the control half enqueues ingests
/// it with zero new code. Bulk API results are already CSV; each page is
/// re-emitted in the source's declared column order (the same
/// record-id/updated-at/mapped order `push_records` renders), which also
/// strips the per-page header repeats.
#[derive(Default)]
pub struct ExtractPull;

impl Task<Session> for ExtractPull {
    const NAME: &'static str = "extract";
    const VERSION: u32 = 1;
    type Input = ();
    type Output = Plan<ExtractReport>;
    const WRITES_LAKE: bool = false;

    fn run(
        &mut self,
        ctx: &mut TaskCtx<'_, Session>,
        (): Self::Input,
    ) -> Result<Outcome<Self::Output>, TaskError> {
        let request = crate::extract_params::ExtractRequest::from_params(&ctx.order.params)
            .map_err(|m| TaskError::Failed(er_errors::ErError::precondition(m)))?;

        // The credential, resolved by the control plane into the order — the
        // pod holds no database credential (§6.4), the `models` precedent.
        let conn = &ctx.order.connection;
        let field = |key: &str| conn.get(key).and_then(serde_json::Value::as_str);
        let connection = match (
            field("instance_url"),
            field("client_id"),
            field("client_secret"),
            field("api_version"),
        ) {
            (Some(instance_url), Some(client_id), Some(client_secret), Some(api_version)) => {
                crate::salesforce::Connection {
                    instance_url: instance_url.to_string(),
                    client_id: client_id.to_string(),
                    client_secret: client_secret.to_string(),
                    api_version: api_version.to_string(),
                }
            }
            _ => {
                return Err(TaskError::Failed(er_errors::ErError::precondition(
                    "the work order carries no salesforce credential — the connection no \
                     longer exists, or this order predates the extract kind",
                )));
            }
        };
        let watermark = match request.mode {
            crate::extract_params::ExtractMode::Incremental => {
                field("watermark").map(str::to_string)
            }
            crate::extract_params::ExtractMode::Full => None,
        };

        // The source's declared CSV shape, from the published config the lake
        // env names — the same file read the control half makes, and the same
        // column order `push_records` renders, so the delivery this stage
        // writes is indistinguishable from an uploaded one.
        let config = published_config(&ctx.order.lake_env)?;
        let Some(spec) = config.sources.get(&request.source) else {
            return Err(TaskError::Failed(er_errors::ErError::precondition(format!(
                "param: source {:?} is not declared in the published config",
                request.source
            ))));
        };
        let columns = crate::extract_params::source_csv_columns(spec);
        let soql = crate::salesforce::build_soql(
            &request.object,
            &columns,
            &spec.updated_at_column,
            watermark.as_deref(),
            request.where_clause.as_deref(),
            request.exclude_converted_leads,
        )
        .map_err(|m| TaskError::Failed(er_errors::ErError::precondition(m)))?;

        let drop_root = ctx.order.drop_root.as_deref().ok_or_else(|| {
            TaskError::Failed(er_errors::ErError::precondition(
                "the dataset has no drop_root, so an extract has nowhere to land",
            ))
        })?;
        let delivery_id = er_ids::new_v7();
        let part = crate::s3_ingest::staging_path(
            std::path::Path::new(drop_root),
            &request.source,
            &delivery_id,
        )
        .ok_or_else(|| {
            TaskError::Failed(er_errors::ErError::precondition(format!(
                "param: source {:?} is not a safe path segment",
                request.source
            )))
        })?;

        let mut file = std::fs::File::create(&part).map_err(|e| {
            engine_error(&format!("i/o error staging into {drop_root}: {e}"))
        })?;
        crate::salesforce::write_header(&columns, &mut file)
            .map_err(|e| engine_error(&format!("i/o error writing the header: {e}")))?;
        let outcome = crate::salesforce::extract_to_writer(
            &connection,
            &soql,
            &columns,
            &spec.updated_at_column,
            &mut file,
        )
        .map_err(|e| {
            // A half-written part file must not survive a failure: the leading
            // dot keeps it from being ingested, but the next try mints a new
            // delivery id and would leave this one behind forever.
            let _ = std::fs::remove_file(&part);
            sf_error(&e)
        })?;
        file.sync_all()
            .map_err(|e| engine_error(&format!("i/o error flushing the delivery: {e}")))?;
        let bytes = i64::try_from(file.metadata().map_or(0, |m| m.len())).unwrap_or(0);
        drop(file);

        let rows = i64::try_from(outcome.rows).unwrap_or(i64::MAX);
        let report = ExtractReport {
            connection_id: request.connection_id.clone(),
            source: request.source.clone(),
            object: request.object.clone(),
            delivery_id: delivery_id.clone(),
            mode: match request.mode {
                crate::extract_params::ExtractMode::Incremental => "incremental".to_string(),
                crate::extract_params::ExtractMode::Full => "full".to_string(),
            },
            rows,
            bytes,
            watermark: outcome.max_updated,
        };
        if outcome.rows == 0 {
            // Nothing changed upstream. The empty page set is a SUCCESS — a
            // scheduled incremental mostly finds nothing — but there is no
            // delivery to publish, so the part file goes and the control half
            // sees `rows: 0` and enqueues nothing.
            let _ = std::fs::remove_file(&part);
            return Ok(Outcome::NoOp(Plan(report)));
        }
        let live = std::path::Path::new(drop_root).join(format!("{}.csv", request.source));
        std::fs::rename(&part, &live)
            .map_err(|e| engine_error(&format!("i/o error publishing the delivery: {e}")))?;
        Ok(Outcome::Done(
            Plan(report),
            er_dag::Counters(serde_json::json!({ "rows": rows, "bytes": bytes })),
        ))
    }
}

/// The published config, from the path the lake env names — a plain file read,
/// the same one the control half's `load_published_config` makes.
fn published_config(lake_env: &serde_json::Value) -> Result<er_config::Config, TaskError> {
    let path = lake_env
        .get("ER_CONFIG_PATH")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            TaskError::Failed(er_errors::ErError::config(
                "org env missing ER_CONFIG_PATH".to_string(),
            ))
        })?;
    let text = std::fs::read_to_string(path).map_err(|e| {
        TaskError::Failed(er_errors::ErError::config(format!(
            "read config {path}: {e}"
        )))
    })?;
    er_config::load_config_str(&text)
        .map_err(|e| TaskError::Failed(er_errors::ErError::config(crate::errfmt::chain(&e))))
}

/// Map a [`crate::salesforce::SfError`] onto the er-errors taxonomy: the
/// variants pre-sorted it, so this is a lookup rather than a message read.
fn sf_error(e: &crate::salesforce::SfError) -> TaskError {
    use crate::salesforce::SfError as E;
    let err = match e {
        E::Auth(_) => er_errors::ErError::config(e.to_string()),
        E::Refused(_) => er_errors::ErError::precondition(e.to_string()),
        E::Transient(_) => er_errors::ErError::stage_failure(e.to_string())
            .with_class(er_errors::ErrorClass::TransientIo),
        E::Protocol(_) => er_errors::ErError::stage_failure(e.to_string()),
    };
    TaskError::Failed(err)
}

/// Classify an engine failure STRING into the er-errors taxonomy.
///
/// The engine hands back a formatted message rather than a typed error at this
/// boundary, so something has to read the text. This is the one place that
/// does it, and it produces an [`er_errors::ErrorClass`] directly.
///
/// It used to go through `pg::FailureClass::classify` — the legacy arm's
/// classifier — which made new-arm code depend on a type the legacy arm owns
/// and blocked that type's deletion. The design's §6.5 says the two taxonomies
/// "collapse into the latter at the executor boundary", and this function IS
/// that boundary, so the rules live here now. `pg::FailureClass` keeps only
/// legacy-arm callers and goes with it in Phase 4.
///
/// The substring rules are carried over unchanged, deliberately: they match the
/// message shapes `er-cli` actually produces, and the table in the tests below
/// pins each one. Order matters — precondition before config before transient,
/// because a message can satisfy more than one (`read config` also contains no
/// transient marker, but `open lake: … timed out` would match two if the
/// transient arm came first).
fn engine_error_class(message: &str) -> er_errors::ErrorClass {
    let m = message.to_ascii_lowercase();
    if m.contains("org env missing")
        || m.contains("no prior run")
        || m.contains("no source deliveries")
        || m.contains("no trained model")
        || m.contains("no model yet")
        || m.contains("has no drop_root")
        // A request that named something invalid — a missing required param,
        // an unknown subject — is a precondition for the same reason the
        // others are: retrying cannot help, because what is missing arrives
        // when a caller changes the request. Matched on `param:` because the
        // validators that produce these messages all name the parameter and
        // then explain, a shape worth keeping stable for exactly this reason.
        || m.contains("param:")
    {
        er_errors::ErrorClass::Precondition
    } else if m.starts_with("config:") || m.contains("read config") {
        er_errors::ErrorClass::Config
    } else if m.contains("open lake")
        || m.contains("read model")
        || m.contains("connection")
        || m.contains("timed out")
        || m.contains("timeout")
        || m.contains("temporarily")
        || m.contains("i/o error")
    {
        er_errors::ErrorClass::TransientIo
    } else {
        // What `pg::FailureClass::Internal` became: the unclassified default,
        // which `er_errors::UNCLASSIFIED` already defines as `Data`. Not a
        // silent widening — `stage_failure` has always carried `Data`, so this
        // is the class that arm produced too.
        er_errors::UNCLASSIFIED
    }
}

/// Map an engine failure string onto the er-errors taxonomy the executor's
/// retry policy reads.
fn engine_error(message: &str) -> TaskError {
    // Every one of these exits `StageFailure`; only the class differs, so the
    // constructor is chosen to match the class rather than the other way
    // round. `precondition` and `config` set it themselves; the other two go
    // through `stage_failure`, which carries `Data`.
    let err = match engine_error_class(message) {
        er_errors::ErrorClass::Precondition => {
            er_errors::ErError::precondition(message.to_string())
        }
        er_errors::ErrorClass::Config => er_errors::ErError::config(message.to_string()),
        class => {
            let e = er_errors::ErError::stage_failure(message.to_string());
            if class == er_errors::UNCLASSIFIED {
                e
            } else {
                e.with_class(class)
            }
        }
    };
    TaskError::Failed(err)
}

#[cfg(test)]
mod engine_error_tests {
    use super::engine_error_class;
    use er_errors::ErrorClass;

    /// The literal strings this codebase produces, and the class each must get.
    ///
    /// This table came from `pg_job_failure.rs`, which pinned the same cases
    /// against `pg::FailureClass`. It moved here with the rules rather than
    /// being deleted with them: the message shapes are what the assertion is
    /// really about, and they outlive whichever type does the classifying.
    #[test]
    fn engine_messages_classify_the_way_the_retry_policy_needs() {
        let cases: &[(&str, ErrorClass)] = &[
            // Preconditions — retrying cannot help, so these must never be
            // retryable or a doomed job burns its whole budget.
            ("org env missing ER_LAKE_CATALOG", ErrorClass::Precondition),
            (
                "no source deliveries found under /app/drop",
                ErrorClass::Precondition,
            ),
            ("no prior run to correct", ErrorClass::Precondition),
            ("no model yet (ER_MODEL_PATH)", ErrorClass::Precondition),
            (
                "org has no drop_root for a run_all",
                ErrorClass::Precondition,
            ),
            ("param: limit must be positive", ErrorClass::Precondition),
            // Config.
            (
                "config: thresholds.auto_merge must be > review_low",
                ErrorClass::Config,
            ),
            ("read config /srv/er/acme.yaml", ErrorClass::Config),
            // Transient — the only family the task budget should spend a try on.
            ("open lake: IO Error: disk full", ErrorClass::TransientIo),
            (
                "read model: Resource temporarily unavailable",
                ErrorClass::TransientIo,
            ),
            ("connection reset by peer", ErrorClass::TransientIo),
            ("operation timed out", ErrorClass::TransientIo),
            // Anything unrecognised is the unclassified default, NOT transient:
            // guessing "retryable" on an unknown failure is how a real bug
            // becomes three identical pod launches.
            ("thread 'main' panicked at src/lib.rs:1", ErrorClass::Data),
            ("", ErrorClass::Data),
        ];
        for (message, want) in cases {
            assert_eq!(
                engine_error_class(message),
                *want,
                "message {message:?} classified wrongly"
            );
        }
    }

    /// The property the table above exists to protect: only `transient_io` is
    /// retryable, so a precondition or a panic must not be.
    #[test]
    fn only_the_transient_family_is_retryable() {
        assert!(ErrorClass::TransientIo.retryable());
        assert!(!engine_error_class("org env missing X").retryable());
        assert!(!engine_error_class("config: bad").retryable());
        assert!(!engine_error_class("panicked").retryable());
    }
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
        let report = er_cli::run_train(
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
            // Convergence travels as a counter, not as an error: EM stopping at
            // the iteration cap still produces usable parameters, and refusing
            // the model would be worse than shipping it with the caveat
            // attached. Until now the caveat was dropped on the floor —
            // `run_train` discarded the whole outcome, so a session that never
            // settled was indistinguishable from one that converged on step
            // two, in the ledger and everywhere downstream of it.
            er_dag::Counters(serde_json::json!({
                "labels_used": labels.len(),
                "em_sessions": report.sessions,
                "em_converged": report.converged(),
                "em_max_iterations": report.max_iterations,
                // Named, not counted: which blocking rule failed to settle is
                // the actionable half — it points at the rule whose pair
                // population is too small or too skewed for EM to resolve.
                "em_not_converged": report.not_converged,
                // Provenance, for the same reason: which levels the model is
                // scoring from a built-in default rather than from this corpus.
                // Not an error — the default is a considered prior — but the
                // saved model looks identical either way, so the ledger is the
                // only place the distinction can survive the run.
                "levels_fully_measured": report.fully_measured(),
                "levels_defaulted": report.defaulted_levels,
                // How loud this corpus actually is, against the one global
                // number a compiled model has to assume. Ledgered because the
                // two kinds of model are used interchangeably as a tenant's
                // active version, so a threshold tuned against one scale can
                // silently mean something else against the other.
                "full_agreement_bits": report.full_agreement_bits,
                "bits_per_field": report.bits_per_field,
                "assumed_scale_diverges": report.scale_divergence().is_some(),
            })),
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

/// Creates and reconciles the new tenant's lake: anything opening a lake is a
/// pod task (§3.5), which is the whole reason `provision` was split in two.
///
/// Idempotent by construction — the reconciler applies only what is absent and
/// ledgers what it applied, so a retry over a lake that already exists is a
/// no-op that still reports the catalog hash.
#[derive(Default)]
pub struct LakeInit;

impl Task<Session> for LakeInit {
    const NAME: &'static str = "lake_init";
    const VERSION: u32 = 1;
    type Input = (Plan<super::plans::CatalogCreated>,);
    type Output = Plan<LakeInitialized>;
    const WRITES_LAKE: bool = true;

    fn run(
        &mut self,
        ctx: &mut TaskCtx<'_, Session>,
        _input: Self::Input,
    ) -> Result<Outcome<Self::Output>, TaskError> {
        // Reconcile through the session this task ALREADY HOLDS.
        //
        // This used to call `crate::schema::reconcile_lake_env`, which opens
        // its own session from the tenant env, reasoning that "the lake this
        // task initialises may not have existed when the pod's ctx session was
        // opened, so the probe handle is not the one to use". That premise is
        // false in this DAG: `create_catalog` is `lake_init`'s upstream and it
        // creates the catalog, so the file exists before this pod starts.
        //
        // The cost of the second handle was the bug (#778). Two sessions
        // attached the same catalog file inside one process, and the ctx
        // session — attached BEFORE the reconcile and dropped AFTER it, since
        // it lives to the end of `exec_task::run_inner` — wrote its own
        // earlier view back over the committed metadata. What survived was the
        // reconciler's parquet data files with no catalog rows pointing at
        // them: a provisioned lake whose relations were all absent, while the
        // task reported success because its `catalog_hash` is computed from
        // the compiled catalog rather than read back.
        let outcome = er_entity_diff::reconcile_lake(
            ctx.session.connection(),
            &er_entities::catalog::catalog(),
            &crate::schema::applied_by(),
        )
        .map_err(|e| match e {
            // The breaking message already lists every difference.
            e @ er_entity_diff::ReconcileError::Breaking(_) => e.to_string(),
            other => format!("lake schema: {}", crate::errfmt::chain(&other)),
        });
        let (status, hash, detail) = crate::schema::tenant_state(&outcome);
        match outcome {
            Ok(_) => Ok(Outcome::Done(
                Plan(LakeInitialized {
                    catalog_hash: hash,
                    status: status.to_string(),
                    detail: detail.clone(),
                }),
                er_dag::Counters(serde_json::json!({ "detail": detail })),
            )),
            Err(message) => Err(engine_error(&message)),
        }
    }
}
