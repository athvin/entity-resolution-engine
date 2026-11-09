//! Cross-module tests over a realistic test DAG: wiring, assembly refusals,
//! the two hashes' behaviour, the readiness table row by row, settle, and the
//! clear closure. The leaf-binary types are stand-ins (`S = ()`, `C = ()`).

use serde::{Deserialize, Serialize};

use crate::assembly::AssemblyError;
use crate::builder::{DagBuilder, NodeId};
use crate::policy::{Backoff, ClassRule, NodePolicy, ResourceClass, TriggerRule};
use crate::readiness::{PassAction, SettleVerdict, TaskState, Verdict, evaluate, pass, settle};
use crate::resume::clear_closure;
use crate::task::{
    ControlCtx, ControlTask, Counters, LakeEdge, LakeRef, Outcome, Plan, PlanDoc,
    ReferenceExistence, Task, TaskCtx, TaskError,
};

// ---- stand-in edge and plan vocabulary ------------------------------------

struct Landed;
impl LakeEdge for Landed {
    const STABLE_NAME: &'static str = "landed";
    const RELATIONS: &'static [&'static str] = &["raw_records"];
}
struct Scored;
impl LakeEdge for Scored {
    const STABLE_NAME: &'static str = "scored";
    const RELATIONS: &'static [&'static str] = &["match_scores"];
}
struct Done;
impl LakeEdge for Done {
    const STABLE_NAME: &'static str = "done";
    const RELATIONS: &'static [&'static str] = &["golden_records"];
}

#[derive(Serialize, Deserialize)]
struct TestPlan {
    mode: String,
}
impl PlanDoc for TestPlan {
    const STABLE_NAME: &'static str = "test_plan";
}

// ---- stand-in tasks --------------------------------------------------------

macro_rules! stub_task {
    ($ty:ident, $name:literal, $input:ty, $output:ty, $writes:literal) => {
        stub_task!($ty, $name, $input, $output, $writes, NodePolicy::default());
    };
    ($ty:ident, $name:literal, $input:ty, $output:ty, $writes:literal, $policy:expr) => {
        #[derive(Default)]
        struct $ty;
        impl Task<()> for $ty {
            const NAME: &'static str = $name;
            const VERSION: u32 = 1;
            type Input = $input;
            type Output = $output;
            const WRITES_LAKE: bool = $writes;
            fn policy() -> NodePolicy {
                $policy
            }
            fn run(
                &mut self,
                _ctx: &mut TaskCtx<'_, ()>,
                _input: Self::Input,
            ) -> Result<Outcome<Self::Output>, TaskError> {
                Err(TaskError::Skip)
            }
        }
    };
}

stub_task!(Init, "init", (), Plan<TestPlan>, false);
stub_task!(Ingest, "ingest", (Plan<TestPlan>,), LakeRef<Landed>, true);
stub_task!(
    Match,
    "match",
    (Plan<TestPlan>, LakeRef<Landed>),
    LakeRef<Scored>,
    true
);
stub_task!(
    Assemble,
    "assemble",
    (Plan<TestPlan>, LakeRef<Scored>),
    LakeRef<Done>,
    true
);
stub_task!(
    Finish,
    "finish",
    (),
    Plan<TestPlan>,
    false,
    NodePolicy {
        teardown: true,
        ..NodePolicy::default()
    }
);

#[derive(Default)]
struct Register;
impl ControlTask<()> for Register {
    const NAME: &'static str = "register";
    const VERSION: u32 = 1;
    type Input = (Plan<TestPlan>,);
    type Output = Plan<TestPlan>;
    // A real control body awaits Postgres; this one has nothing to await, so
    // the future is spelled out rather than written as an `async fn` clippy
    // would then call pointless.
    fn run(
        &mut self,
        _ctx: &mut ControlCtx<'_, ()>,
        _input: Self::Input,
    ) -> impl std::future::Future<Output = Result<Outcome<Self::Output>, TaskError>> + Send {
        std::future::ready(Ok(Outcome::Done(
            Plan(TestPlan { mode: "x".into() }),
            Counters::default(),
        )))
    }
}

/// init → ingest → match → assemble; finish (all_terminal, teardown).
fn linear_dag() -> crate::Dag {
    let mut d: DagBuilder<(), ()> = DagBuilder::new("run_all");
    let plan = d.source::<Init>();
    let raw = d.then::<Ingest>((plan,));
    let scored = d.then::<Match>((plan, raw));
    let done = d.then::<Assemble>((plan, scored));
    d.epilogue::<Finish>(&[done.id()], TriggerRule::AllTerminal);
    d.build().expect("the linear DAG assembles")
}

#[test]
fn wiring_compiles_and_assembles_with_sane_metadata() {
    let dag = linear_dag();
    assert_eq!(dag.nodes.len(), 5);
    assert_eq!(dag.nodes[0].task_id, "init");
    assert_eq!(dag.nodes[2].input_names, vec!["test_plan", "landed"]);
    assert_eq!(dag.nodes[2].output_name, "scored");
    assert!(dag.nodes[2].writes_lake);
    assert!(dag.nodes[4].teardown);
    assert_eq!(dag.nodes[4].trigger_rule, TriggerRule::AllTerminal);
    // Registration order is topological; a control task records its kind.
    let mut d: DagBuilder<(), ()> = DagBuilder::new("train");
    let plan = d.source::<Init>();
    let reg = d.control::<Register>((plan,));
    let dag = d.build().expect("assembles");
    assert_eq!(dag.nodes[reg.id().index()].kind.as_str(), "control");
}

#[test]
fn assembly_refusals() {
    // Duplicate task name.
    let mut d: DagBuilder<(), ()> = DagBuilder::new("dup");
    let _ = d.source::<Init>();
    let _ = d.source::<Init>();
    assert_eq!(
        d.build().unwrap_err(),
        AssemblyError::DuplicateTaskName { task_id: "init" }
    );

    // Empty graph.
    let d: DagBuilder<(), ()> = DagBuilder::new("empty");
    assert_eq!(d.build().unwrap_err(), AssemblyError::Empty);

    // Two lake writers with no path between them.
    stub_task!(ForkA, "fork_a", (Plan<TestPlan>,), LakeRef<Landed>, true);
    stub_task!(ForkB, "fork_b", (Plan<TestPlan>,), LakeRef<Scored>, true);
    let mut d: DagBuilder<(), ()> = DagBuilder::new("forked");
    let plan = d.source::<Init>();
    let _ = d.then::<ForkA>((plan,));
    let _ = d.then::<ForkB>((plan,));
    assert_eq!(
        d.build().unwrap_err(),
        AssemblyError::UnorderedLakeWriters {
            a: "fork_a",
            b: "fork_b"
        }
    );

    // escalate_on_oom on a fixed-L task has no next class.
    stub_task!(
        BigMatch,
        "big_match",
        (Plan<TestPlan>,),
        LakeRef<Scored>,
        true,
        NodePolicy {
            class: ClassRule::Fixed(ResourceClass::L),
            escalate_on_oom: true,
            ..NodePolicy::default()
        }
    );
    let mut d: DagBuilder<(), ()> = DagBuilder::new("oom");
    let plan = d.source::<Init>();
    let _ = d.then::<BigMatch>((plan,));
    assert_eq!(
        d.build().unwrap_err(),
        AssemblyError::EscalateOnFixedL {
            task_id: "big_match"
        }
    );

    // A fusion segment must be contiguous in topological order.
    stub_task!(
        SegHead,
        "seg_head",
        (Plan<TestPlan>,),
        LakeRef<Landed>,
        true,
        NodePolicy {
            segment: Some("s1"),
            ..NodePolicy::default()
        }
    );
    stub_task!(
        Between,
        "between",
        (LakeRef<Landed>,),
        LakeRef<Scored>,
        true
    );
    stub_task!(
        SegTail,
        "seg_tail",
        (LakeRef<Scored>,),
        LakeRef<Done>,
        true,
        NodePolicy {
            segment: Some("s1"),
            ..NodePolicy::default()
        }
    );
    let mut d: DagBuilder<(), ()> = DagBuilder::new("gap");
    let plan = d.source::<Init>();
    let a = d.then::<SegHead>((plan,));
    let b = d.then::<Between>((a,));
    let _ = d.then::<SegTail>((b,));
    assert_eq!(
        d.build().unwrap_err(),
        AssemblyError::SegmentNotContiguous { segment: "s1" }
    );
}

#[test]
fn foreign_handles_are_rejected_even_when_the_index_exists() {
    let mut first: DagBuilder<(), ()> = DagBuilder::new("first");
    let local = first.source::<Init>();
    let mut second: DagBuilder<(), ()> = DagBuilder::new("second");
    let foreign = second.source::<Init>();
    assert_eq!(local.id().index(), foreign.id().index());
    let _ = first.then::<Ingest>((foreign,));
    assert_eq!(
        first.build().unwrap_err(),
        AssemblyError::ForeignUpstream { task_id: "ingest" }
    );

    let mut first: DagBuilder<(), ()> = DagBuilder::new("first_control");
    let _ = first.source::<Init>();
    let _ = first.control::<Register>((foreign,));
    assert_eq!(
        first.build().unwrap_err(),
        AssemblyError::ForeignUpstream {
            task_id: "register"
        }
    );

    let mut first: DagBuilder<(), ()> = DagBuilder::new("first_epilogue");
    let _ = first.source::<Init>();
    let _ = first.epilogue::<Finish>(&[foreign.id()], TriggerRule::AllTerminal);
    assert_eq!(
        first.build().unwrap_err(),
        AssemblyError::ForeignUpstream { task_id: "finish" }
    );
}

#[test]
fn invalid_upstream_indices_are_rejected_before_readiness() {
    let dag = linear_dag();
    let mut nodes = dag.nodes;
    let owner = nodes[1].upstream[0].builder_id;
    let out_of_range = nodes.len();
    nodes[1].upstream[0] = NodeId {
        index: out_of_range,
        builder_id: owner,
    };
    assert_eq!(
        crate::assembly::assemble("out_of_range".into(), nodes).unwrap_err(),
        AssemblyError::InvalidUpstream {
            task_id: "ingest",
            upstream: 5
        }
    );

    let dag = linear_dag();
    let mut nodes = dag.nodes;
    nodes[1].upstream[0].index = 1;
    assert_eq!(
        crate::assembly::assemble("self_edge".into(), nodes).unwrap_err(),
        AssemblyError::InvalidUpstream {
            task_id: "ingest",
            upstream: 1
        }
    );
}

#[test]
fn the_two_hashes_split_structure_from_policy() {
    let base = linear_dag();
    let again = linear_dag();
    assert_eq!(base.structural_fingerprint, again.structural_fingerprint);
    assert_eq!(base.policy_hash, again.policy_hash);

    // A policy-only change moves the policy hash and leaves the structure.
    stub_task!(
        SlowMatch,
        "match",
        (Plan<TestPlan>, LakeRef<Landed>),
        LakeRef<Scored>,
        true,
        NodePolicy {
            backoff: Backoff {
                base_secs: 60,
                cap_secs: 900
            },
            ..NodePolicy::default()
        }
    );
    let mut d: DagBuilder<(), ()> = DagBuilder::new("run_all");
    let plan = d.source::<Init>();
    let raw = d.then::<Ingest>((plan,));
    let scored = d.then::<SlowMatch>((plan, raw));
    let done = d.then::<Assemble>((plan, scored));
    d.epilogue::<Finish>(&[done.id()], TriggerRule::AllTerminal);
    let slower = d.build().expect("assembles");
    assert_eq!(base.structural_fingerprint, slower.structural_fingerprint);
    assert_ne!(base.policy_hash, slower.policy_hash);

    // Dropping an edge moves the structural fingerprint.
    stub_task!(
        LooseMatch,
        "match",
        (Plan<TestPlan>,),
        LakeRef<Scored>,
        true
    );
    stub_task!(
        LooseAssemble,
        "assemble",
        (LakeRef<Scored>,),
        LakeRef<Done>,
        true
    );
    stub_task!(
        LooseIngest,
        "ingest",
        (Plan<TestPlan>,),
        LakeRef<Landed>,
        false
    );
    let mut d: DagBuilder<(), ()> = DagBuilder::new("run_all");
    let plan = d.source::<Init>();
    let _ = d.then::<LooseIngest>((plan,));
    let scored = d.then::<LooseMatch>((plan,));
    let done = d.then::<LooseAssemble>((scored,));
    d.epilogue::<Finish>(&[done.id()], TriggerRule::AllTerminal);
    let looser = d.build().expect("assembles");
    assert_ne!(base.structural_fingerprint, looser.structural_fingerprint);
}

#[test]
fn the_readiness_table_row_by_row() {
    use TaskState::{Canceled, Failed, Running, Skipped, Succeeded};

    // all_succeeded.
    let r = TriggerRule::AllSucceeded;
    assert_eq!(evaluate(r, &[]), Verdict::Fire);
    assert_eq!(evaluate(r, &[Succeeded, Succeeded]), Verdict::Fire);
    assert_eq!(evaluate(r, &[Succeeded, Running]), Verdict::Wait);
    assert_eq!(
        evaluate(r, &[Succeeded, Failed]),
        Verdict::Propagate(TaskState::UpstreamFailed)
    );
    assert_eq!(
        evaluate(r, &[Succeeded, Skipped]),
        Verdict::Propagate(TaskState::UpstreamSkipped)
    );
    assert_eq!(
        evaluate(r, &[Succeeded, Canceled]),
        Verdict::Propagate(TaskState::Canceled)
    );
    // Mixed non-success classes are failure-like.
    assert_eq!(
        evaluate(r, &[Skipped, Canceled, Failed]),
        Verdict::Propagate(TaskState::UpstreamFailed)
    );

    // all_terminal fires on any terminal mix, never propagates.
    let r = TriggerRule::AllTerminal;
    assert_eq!(evaluate(r, &[Failed, Canceled, Succeeded]), Verdict::Fire);
    assert_eq!(evaluate(r, &[Failed, Running]), Verdict::Wait);

    // any_failed.
    let r = TriggerRule::AnyFailed;
    assert_eq!(evaluate(r, &[Succeeded, Failed]), Verdict::Fire);
    assert_eq!(
        evaluate(r, &[Succeeded, Succeeded]),
        Verdict::Propagate(TaskState::UpstreamSkipped)
    );
    assert_eq!(evaluate(r, &[Running]), Verdict::Wait);
}

#[test]
fn a_failure_deadens_the_chain_in_one_pass_and_the_job_settles_failed() {
    let dag = linear_dag();
    // init succeeded, ingest failed terminally; match/assemble pending.
    let states = [
        TaskState::Succeeded,
        TaskState::Failed,
        TaskState::None,
        TaskState::None,
        TaskState::None,
    ];
    let actions = pass(&dag, &states);
    assert_eq!(
        actions,
        vec![
            (2, PassAction::Propagate(TaskState::UpstreamFailed)),
            (3, PassAction::Propagate(TaskState::UpstreamFailed)),
            // finish is all_terminal: every upstream terminal after the
            // cascade, so it fires in the same sweep.
            (4, PassAction::Schedule),
        ]
    );

    // Once finish ran, the job settles failed from the non-teardown leaf.
    let settled = [
        TaskState::Succeeded,
        TaskState::Failed,
        TaskState::UpstreamFailed,
        TaskState::UpstreamFailed,
        TaskState::Succeeded,
    ];
    assert_eq!(
        settle(&dag, &settled),
        SettleVerdict::Settled(TaskState::Failed)
    );

    // A fully green run settles succeeded.
    let green = [TaskState::Succeeded; 5];
    assert_eq!(
        settle(&dag, &green),
        SettleVerdict::Settled(TaskState::Succeeded)
    );

    // A running row is not settleable.
    let mut busy = green;
    busy[2] = TaskState::Running;
    assert_eq!(settle(&dag, &busy), SettleVerdict::NotYet);

    // A pending row with every upstream terminal that survived a pass is the
    // deadlock branch.
    let mut stuck = green;
    stuck[3] = TaskState::None;
    assert_eq!(settle(&dag, &stuck), SettleVerdict::Deadlock(3));
}

#[test]
fn the_clear_closure_closes_down_and_demands_up() {
    let dag = linear_dag();
    let all_present = |_: usize| ReferenceExistence::Present;

    // Clearing match re-runs match + assemble (downstream closure; finish is
    // downstream of assemble and re-runs too).
    let plan = clear_closure(&dag, &[2], true, all_present);
    assert_eq!(
        plan.must_run.iter().copied().collect::<Vec<_>>(),
        vec![2, 3, 4]
    );
    assert!(plan.demanded_upstream.is_empty());

    // If ingest's snapshot was expired, clearing match drafts ingest in — and
    // ingest's own inputs are then probed too (init's plan is present).
    let expired_ingest = |idx: usize| {
        if idx == 1 {
            ReferenceExistence::Missing
        } else {
            ReferenceExistence::Present
        }
    };
    let plan = clear_closure(&dag, &[2], true, expired_ingest);
    assert_eq!(
        plan.must_run.iter().copied().collect::<Vec<_>>(),
        vec![1, 2, 3, 4]
    );
    assert_eq!(
        plan.demanded_upstream.iter().copied().collect::<Vec<_>>(),
        vec![1]
    );

    // include_downstream = false touches the seed alone.
    let plan = clear_closure(&dag, &[1], false, all_present);
    assert_eq!(plan.must_run.iter().copied().collect::<Vec<_>>(), vec![1]);
}
