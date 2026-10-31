//! The readiness evaluator (§5.3): a table-driven pure function over the
//! compiled edges and the rows' states. SQL records its verdicts (the guarded
//! `mark_scheduled` and `propagate` statements); this is the authority.

use crate::assembly::Dag;
use crate::policy::TriggerRule;

/// The closed task-instance state set (`task_instances.state`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskState {
    None,
    Scheduled,
    Queued,
    Running,
    UpForRetry,
    Restarting,
    Canceling,
    Succeeded,
    Skipped,
    Failed,
    TimedOut,
    UpstreamFailed,
    UpstreamSkipped,
    Canceled,
}

/// The state classes of §5.3's terminal taxonomy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateClass {
    SuccessLike,
    SkipLike,
    FailureLike,
    StopLike,
    Pending,
}

impl TaskState {
    pub const ALL: [TaskState; 14] = [
        TaskState::None,
        TaskState::Scheduled,
        TaskState::Queued,
        TaskState::Running,
        TaskState::UpForRetry,
        TaskState::Restarting,
        TaskState::Canceling,
        TaskState::Succeeded,
        TaskState::Skipped,
        TaskState::Failed,
        TaskState::TimedOut,
        TaskState::UpstreamFailed,
        TaskState::UpstreamSkipped,
        TaskState::Canceled,
    ];

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            TaskState::None => "none",
            TaskState::Scheduled => "scheduled",
            TaskState::Queued => "queued",
            TaskState::Running => "running",
            TaskState::UpForRetry => "up_for_retry",
            TaskState::Restarting => "restarting",
            TaskState::Canceling => "canceling",
            TaskState::Succeeded => "succeeded",
            TaskState::Skipped => "skipped",
            TaskState::Failed => "failed",
            TaskState::TimedOut => "timed_out",
            TaskState::UpstreamFailed => "upstream_failed",
            TaskState::UpstreamSkipped => "upstream_skipped",
            TaskState::Canceled => "canceled",
        }
    }

    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        TaskState::ALL.into_iter().find(|t| t.as_str() == s)
    }

    #[must_use]
    pub fn class(self) -> StateClass {
        match self {
            TaskState::Succeeded => StateClass::SuccessLike,
            TaskState::Skipped | TaskState::UpstreamSkipped => StateClass::SkipLike,
            TaskState::Failed | TaskState::TimedOut | TaskState::UpstreamFailed => {
                StateClass::FailureLike
            }
            TaskState::Canceled => StateClass::StopLike,
            TaskState::None
            | TaskState::Scheduled
            | TaskState::Queued
            | TaskState::Running
            | TaskState::UpForRetry
            | TaskState::Restarting
            | TaskState::Canceling => StateClass::Pending,
        }
    }

    #[must_use]
    pub fn is_terminal(self) -> bool {
        self.class() != StateClass::Pending
    }
}

/// One rule evaluation (§5.3's table).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// The rule fired: the node may be scheduled.
    Fire,
    /// The rule can never fire: propagate this terminal state.
    Propagate(TaskState),
    /// Some upstream is not yet terminal.
    Wait,
}

/// Evaluate one trigger rule, only once every upstream is terminal (a node
/// with no upstream fires immediately).
#[must_use]
pub fn evaluate(rule: TriggerRule, upstream: &[TaskState]) -> Verdict {
    if upstream.iter().any(|s| !s.is_terminal()) {
        return Verdict::Wait;
    }
    match rule {
        TriggerRule::AllSucceeded => {
            let non_success: Vec<StateClass> = upstream
                .iter()
                .map(|s| s.class())
                .filter(|c| *c != StateClass::SuccessLike)
                .collect();
            if non_success.is_empty() {
                Verdict::Fire
            } else if non_success.iter().all(|c| *c == StateClass::SkipLike) {
                Verdict::Propagate(TaskState::UpstreamSkipped)
            } else if non_success.iter().all(|c| *c == StateClass::StopLike) {
                Verdict::Propagate(TaskState::Canceled)
            } else {
                Verdict::Propagate(TaskState::UpstreamFailed)
            }
        }
        TriggerRule::AllTerminal => Verdict::Fire,
        TriggerRule::AnyFailed => {
            if upstream
                .iter()
                .any(|s| s.class() == StateClass::FailureLike)
            {
                Verdict::Fire
            } else {
                // dagr writes `skipped`; here the propagated form, since
                // nothing ran (§5.3).
                Verdict::Propagate(TaskState::UpstreamSkipped)
            }
        }
    }
}

/// What one readiness pass decided for one node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PassAction {
    /// Record `mark_scheduled` for this node.
    Schedule,
    /// Record `propagate($state)` for this node.
    Propagate(TaskState),
}

/// One readiness sweep over a job's live states (indexed by node): verdicts in
/// topological order, with propagations cascading inside the same pass so a
/// failure deadens a chain in one sweep (§5.3).
#[must_use]
pub fn pass(dag: &Dag, states: &[TaskState]) -> Vec<(usize, PassAction)> {
    assert_eq!(
        states.len(),
        dag.nodes.len(),
        "one live state per compiled node"
    );
    let mut effective: Vec<TaskState> = states.to_vec();
    let mut actions = Vec::new();
    for (idx, node) in dag.nodes.iter().enumerate() {
        if effective[idx] != TaskState::None {
            continue;
        }
        let upstream: Vec<TaskState> = node.upstream.iter().map(|u| effective[u.index()]).collect();
        match evaluate(node.trigger_rule, &upstream) {
            Verdict::Fire => actions.push((idx, PassAction::Schedule)),
            Verdict::Propagate(state) => {
                effective[idx] = state;
                actions.push((idx, PassAction::Propagate(state)));
            }
            Verdict::Wait => {}
        }
    }
    actions
}

/// How a settle check ends (§5.3 "Job settle").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettleVerdict {
    /// A live row is still pending.
    NotYet,
    /// Every row terminal: the job's outcome state (`succeeded | failed |
    /// canceled`), derived from the non-teardown leaves.
    Settled(TaskState),
    /// A pending `none` row survived a pass with every upstream terminal — a
    /// bug; settle the job failed/internal (Airflow's deadlock branch).
    Deadlock(usize),
}

/// Settle a job from its live row states: only once nothing is pending, from
/// the non-teardown leaves — `failed` if any is failure-like, `canceled` if
/// any is stop-like and none failure-like, else `succeeded`.
#[must_use]
pub fn settle(dag: &Dag, states: &[TaskState]) -> SettleVerdict {
    assert_eq!(
        states.len(),
        dag.nodes.len(),
        "one live state per compiled node"
    );
    for (idx, state) in states.iter().enumerate() {
        if state.is_terminal() {
            continue;
        }
        if *state == TaskState::None
            && dag.nodes[idx]
                .upstream
                .iter()
                .all(|u| states[u.index()].is_terminal())
        {
            return SettleVerdict::Deadlock(idx);
        }
        return SettleVerdict::NotYet;
    }
    // A leaf of the graph minus its teardown nodes: `assemble` stays a leaf
    // even though the teardown `finish` hangs off it.
    let leaves: Vec<usize> = (0..dag.nodes.len())
        .filter(|&i| {
            !dag.nodes[i].teardown && dag.downstream_of(i).iter().all(|&d| dag.nodes[d].teardown)
        })
        .collect();
    let classes: Vec<StateClass> = leaves.iter().map(|&i| states[i].class()).collect();
    if classes.contains(&StateClass::FailureLike) {
        SettleVerdict::Settled(TaskState::Failed)
    } else if classes.contains(&StateClass::StopLike) {
        SettleVerdict::Settled(TaskState::Canceled)
    } else {
        SettleVerdict::Settled(TaskState::Succeeded)
    }
}
