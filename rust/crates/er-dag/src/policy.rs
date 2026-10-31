//! Per-node execution policy: retries, backoff, timeout, resource class,
//! fusion segment. Policy feeds the policy hash (never the structural
//! fingerprint), so changing it never refuses a resume (§3.4).

use serde::{Deserialize, Serialize};

/// The D-SIZING resource classes. The class decides the pod's requests,
/// `ER_DUCKDB_THREADS`/`ER_DUCKDB_MEMORY_LIMIT` and the spill budget in one
/// template operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ResourceClass {
    S,
    M,
    L,
}

impl ResourceClass {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            ResourceClass::S => "S",
            ResourceClass::M => "M",
            ResourceClass::L => "L",
        }
    }

    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "S" => Some(ResourceClass::S),
            "M" => Some(ResourceClass::M),
            "L" => Some(ResourceClass::L),
            _ => None,
        }
    }

    /// The OOM escalation ladder: S → M → L; a try already at L is terminal
    /// `resources` (§5.5).
    #[must_use]
    pub fn escalated(self) -> Option<Self> {
        match self {
            ResourceClass::S => Some(ResourceClass::M),
            ResourceClass::M => Some(ResourceClass::L),
            ResourceClass::L => None,
        }
    }
}

/// How a node's class is resolved at planning (§3.7): pinned by the author, or
/// from the run (an operator override on the job row, else the tenant's last
/// succeeded `init` output through the D-SIZING thresholds, else M).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClassRule {
    Fixed(ResourceClass),
    FromRun,
}

/// Exponential backoff parameters; the factor is 2 and the deterministic
/// hash jitter in `[min, 2·min)` lives in the retry statement itself (§5.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Backoff {
    pub base_secs: u32,
    pub cap_secs: u32,
}

/// dagr's three trigger rules (§5.3). Only `all_succeeded` (the default, the
/// only rule a data-dependent node can carry) and `all_terminal` (`finish`)
/// have consumers in v1; `any_failed` is reserved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriggerRule {
    AllSucceeded,
    AllTerminal,
    AnyFailed,
}

impl TriggerRule {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            TriggerRule::AllSucceeded => "all_succeeded",
            TriggerRule::AllTerminal => "all_terminal",
            TriggerRule::AnyFailed => "any_failed",
        }
    }
}

/// Author-declared execution policy for one node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodePolicy {
    /// Tries beyond the first; default 2 (= `jobs.max_attempts` 3 today).
    pub retries: u32,
    pub backoff: Backoff,
    /// → `Job.activeDeadlineSeconds`; `None` → the class default.
    pub timeout_secs: Option<u64>,
    /// Ignored for a control task (its row carries no class).
    pub class: ClassRule,
    /// Retry an OOM-killed try at the next class up (§5.5).
    pub escalate_on_oom: bool,
    /// Fusion segment (§3.7); policy hash only.
    pub segment: Option<&'static str>,
    /// Runs under a canceling job too (`finish`).
    pub teardown: bool,
    /// DuckDB threads override for a stage that is not order-stable above one
    /// thread (§9.3); `None` → the class's setting.
    pub threads: Option<u32>,
}

impl Default for NodePolicy {
    fn default() -> Self {
        NodePolicy {
            retries: 2,
            backoff: Backoff {
                base_secs: 30,
                cap_secs: 900,
            },
            timeout_secs: None,
            class: ClassRule::FromRun,
            escalate_on_oom: false,
            segment: None,
            teardown: false,
            threads: None,
        }
    }
}

impl NodePolicy {
    /// The control-task default (§3.6): retries 2, backoff 5 s / 60 s,
    /// timeout 60 s, no class, no segment.
    #[must_use]
    pub fn control() -> Self {
        NodePolicy {
            backoff: Backoff {
                base_secs: 5,
                cap_secs: 60,
            },
            timeout_secs: Some(60),
            ..NodePolicy::default()
        }
    }
}
