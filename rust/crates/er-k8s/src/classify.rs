//! The §6.5 classifiers, pure and row-tested: platform evidence in, a verdict
//! out. The dispatcher turns verdicts into the §4.5 statements; nothing here
//! touches Postgres. Two deliberate properties carried from the design:
//! **exit codes are evidence, never classifier inputs** (exit 137 without an
//! `OOMKilled` reason is a relaunch, never an inferred OOM), and **a verdict
//! that ends in a relaunch is written only on terminal-phase or DELETED
//! evidence** (clause d — a foreground delete returns at the deletion
//! timestamp, and a process stalled on EFS I/O can outlive SIGKILL).

use crate::api::{JobSnapshot, PodSnapshot};
use crate::record::{TerminalRecord, TerminalState};

/// The fatal-waiting and no-pod bound (§5.7).
pub const FATAL_WAIT_BOUND_SECS: u64 = 60;
/// The unschedulable bound: NodeClaim → Ready measures 14–18 s, but a
/// quota-starved account leaves a pod Pending indefinitely (§5.7).
pub const UNSCHEDULABLE_BOUND_SECS: u64 = 600;
/// A Job with no pod and no event after this bound spends a launch.
pub const NO_POD_BOUND_SECS: u64 = 60;
/// A terminal pod with no terminal record after this bound is classified
/// without one.
pub const NO_RECORD_BOUND_SECS: u64 = 60;

/// The container waiting reasons that can never recover (§5.5).
pub const FATAL_WAITING_REASONS: [&str; 4] = [
    "ImagePullBackOff",
    "ErrImagePull",
    "CreateContainerConfigError",
    "InvalidImageName",
];

/// The row states the terminal classifier dispatches on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowState {
    Queued,
    Running,
    Restarting,
    Canceling,
}

/// A pre-start verdict (§6.5's first table). `Wait` is "the bound has not
/// passed"; both others delete the Job first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreStartVerdict {
    Wait,
    /// Spend a launch (`requeue_launch`); the reason lands on the row.
    RequeueLaunch {
        reason: String,
        detail: Option<String>,
    },
    /// Spend nothing (`capacity_wait`): a starved account is the operator's
    /// problem, never the tenant's.
    CapacityWait {
        reason: String,
        detail: Option<String>,
    },
}

/// Classify a pre-start pod (phase Pending, or no pod at all).
///
/// `fatal_waiting_secs` / `unschedulable_secs` / `no_pod_secs` are how long
/// the respective evidence has been standing — the caller's clocks, so this
/// stays pure.
#[must_use]
pub fn pre_start(
    pod: Option<&PodSnapshot>,
    job: &JobSnapshot,
    newest_failed_create: Option<&str>,
    fatal_waiting_secs: u64,
    unschedulable_secs: u64,
    no_pod_secs: u64,
) -> PreStartVerdict {
    if let Some(pod) = pod {
        if let Some(reason) = pod
            .waiting_reason
            .as_deref()
            .filter(|r| FATAL_WAITING_REASONS.contains(r))
        {
            if fatal_waiting_secs >= FATAL_WAIT_BOUND_SECS {
                return PreStartVerdict::RequeueLaunch {
                    reason: reason.to_string(),
                    detail: None,
                };
            }
            return PreStartVerdict::Wait;
        }
        if pod.unschedulable {
            if unschedulable_secs >= UNSCHEDULABLE_BOUND_SECS {
                return PreStartVerdict::CapacityWait {
                    reason: "Unschedulable".to_string(),
                    detail: pod.scheduling_message.clone(),
                };
            }
            return PreStartVerdict::Wait;
        }
        return PreStartVerdict::Wait;
    }
    // No pod at all: the Job controller could not create one.
    if job.active == 0 && no_pod_secs >= NO_POD_BOUND_SECS {
        return match newest_failed_create {
            // The one deliberate message-text match in the design: compute
            // quota is charged to the pod the controller tries to create, and
            // the Event's message is the only evidence.
            Some(message) if message.contains("exceeded quota") => PreStartVerdict::CapacityWait {
                reason: "quota".to_string(),
                detail: Some(message.to_string()),
            },
            Some(message) => PreStartVerdict::RequeueLaunch {
                reason: "FailedCreate".to_string(),
                detail: Some(message.to_string()),
            },
            None => PreStartVerdict::RequeueLaunch {
                reason: "no_pod_created".to_string(),
                detail: None,
            },
        };
    }
    PreStartVerdict::Wait
}

/// A terminal verdict (§6.5's second table), named by the statement the
/// dispatcher runs for it.
#[derive(Debug, Clone, PartialEq)]
pub enum TerminalVerdict {
    /// The pod's evidence is not terminal yet (clause d): do nothing.
    Wait,
    /// The row already has its verdict; store the observed exit/reason
    /// (`record_evidence`) and tombstone.
    RecordEvidence {
        exit_code: Option<i64>,
        reason: Option<String>,
    },
    /// A parsed terminal record decides: `finish`/`fail_or_retry` per its
    /// state and class.
    ApplyRecord(TerminalRecord),
    /// Terminal, no record, row was running: `failed`/`internal`, never a
    /// success and never a retryable class (§6.5).
    FailNoRecord,
    /// Job condition `Failed/DeadlineExceeded`: `timed_out`, terminal.
    TimedOut,
    /// OOMKilled, or the kubelet's local-storage eviction: `resources`,
    /// escalate per policy.
    FailResources {
        reason: String,
        detail: Option<String>,
    },
    /// Relaunch on the infrastructure budget (`requeue_launch`).
    Requeue { reason: String },
    /// `canceling` row, pod provably gone: `settle_canceled`.
    SettleCanceled {
        exit_code: Option<i64>,
        reason: Option<String>,
    },
    /// `restarting` row, pod provably gone: `complete_restart` — recorded,
    /// never classified (the operator's clear wins whatever the pod reported).
    CompleteRestart {
        exit_code: Option<i64>,
        reason: Option<String>,
    },
}

/// What the dispatcher knows about the row when evidence arrives.
#[derive(Debug, Clone, Copy)]
pub struct RowView {
    pub state: RowState,
    /// A `finish` report already landed (the row is `succeeded`/`skipped`).
    pub finish_landed: bool,
}

/// Classify terminal-or-deleted pod evidence for one attempt.
///
/// `deleted` means the watch delivered DELETED (or the resync found the pod
/// gone); `ours` means the delete carried our `owner: revoked` label.
#[must_use]
pub fn terminal(
    row: RowView,
    pod: Option<&PodSnapshot>,
    job: &JobSnapshot,
    deleted: bool,
    no_record_secs: u64,
) -> TerminalVerdict {
    // (a) a landed finish outranks every later pod evidence: annotate, never
    // revoke.
    if row.finish_landed {
        return TerminalVerdict::RecordEvidence {
            exit_code: pod.and_then(|p| p.terminated_exit_code).map(i64::from),
            reason: pod.and_then(|p| p.terminated_reason.clone()),
        };
    }

    // (d) a relaunch-shaped verdict needs the pod provably gone or terminal.
    let terminal_evidence = deleted || pod.is_some_and(PodSnapshot::is_terminal);
    if !terminal_evidence {
        // One exception needs no pod at all: the Job's own deadline condition
        // is control-plane-independent evidence.
        if job.failed_deadline && pod.is_none() {
            return match row.state {
                RowState::Canceling => TerminalVerdict::SettleCanceled {
                    exit_code: None,
                    reason: Some("DeadlineExceeded".into()),
                },
                RowState::Restarting => TerminalVerdict::CompleteRestart {
                    exit_code: None,
                    reason: Some("DeadlineExceeded".into()),
                },
                _ => TerminalVerdict::TimedOut,
            };
        }
        return TerminalVerdict::Wait;
    }

    let exit_code = pod.and_then(|p| p.terminated_exit_code).map(i64::from);
    let reason = pod.and_then(|p| p.terminated_reason.clone());
    let record = pod
        .and_then(|p| p.termination_message.as_deref())
        .and_then(TerminalRecord::parse);

    // By row state first: a canceling or restarting row takes the operator's
    // meaning, whatever the pod said.
    match row.state {
        RowState::Canceling => {
            return TerminalVerdict::SettleCanceled { exit_code, reason };
        }
        RowState::Restarting => {
            return TerminalVerdict::CompleteRestart { exit_code, reason };
        }
        RowState::Queued | RowState::Running => {}
    }

    // An interrupted record defers to the platform's evidence (§5.6): the pod
    // cannot know whether its SIGTERM was a cancel, a deadline, or a drain.
    if record
        .as_ref()
        .is_some_and(|r| r.state == TerminalState::Interrupted)
    {
        if job.failed_deadline {
            return TerminalVerdict::TimedOut;
        }
        if let Some(target) = pod.and_then(|p| p.disruption_target.clone()) {
            return TerminalVerdict::Requeue { reason: target };
        }
        if deleted || pod.and_then(|p| p.status_reason.as_deref()) == Some("NodeLost") {
            return TerminalVerdict::Requeue {
                reason: "vanished".into(),
            };
        }
        return TerminalVerdict::Requeue {
            reason: "sigterm".into(),
        };
    }

    // A record that claims control-plane unreachability: no try ran.
    if record
        .as_ref()
        .and_then(|r| r.termination_reason.as_deref())
        == Some("control_plane_unreachable")
    {
        return TerminalVerdict::Requeue {
            reason: "control_plane_unreachable".into(),
        };
    }

    // A parsed record is the verdict.
    if let Some(record) = record {
        return TerminalVerdict::ApplyRecord(record);
    }

    // No record. Platform evidence decides.
    if reason.as_deref() == Some("OOMKilled") {
        return TerminalVerdict::FailResources {
            reason: "OOMKilled".into(),
            detail: None,
        };
    }
    if let Some(p) = pod {
        // The kubelet's local-storage eviction sets Evicted WITHOUT a
        // DisruptionTarget: this task's spill exceeded its class (§6.5).
        if p.status_reason.as_deref() == Some("Evicted") && p.disruption_target.is_none() {
            return TerminalVerdict::FailResources {
                reason: "Evicted".into(),
                detail: p.termination_message.clone(),
            };
        }
        if let Some(target) = p.disruption_target.clone() {
            return TerminalVerdict::Requeue { reason: target };
        }
    }
    if job.failed_deadline {
        return TerminalVerdict::TimedOut;
    }
    // Exit 137 with no OOM reason and no DisruptionTarget: a SIGKILL whose
    // author we cannot see — never inferred as OOM.
    if exit_code == Some(137) {
        return TerminalVerdict::Requeue {
            reason: "sigkill".into(),
        };
    }
    if deleted {
        // Node gone / manual delete while the row was live, with no record and
        // not ours (`revoked` rows left through the state match above).
        return TerminalVerdict::Requeue {
            reason: pod
                .and_then(|p| p.status_reason.clone())
                .unwrap_or_else(|| "vanished".into()),
        };
    }
    // Terminal pod, no record, no decisive platform evidence.
    match row.state {
        RowState::Running => {
            if no_record_secs >= NO_RECORD_BOUND_SECS {
                TerminalVerdict::FailNoRecord
            } else {
                TerminalVerdict::Wait
            }
        }
        RowState::Queued => TerminalVerdict::Requeue {
            reason: "no_terminal_record".into(),
        },
        RowState::Restarting | RowState::Canceling => unreachable!("handled above"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::PodPhase;

    fn job() -> JobSnapshot {
        JobSnapshot::default()
    }

    fn pod(phase: PodPhase) -> PodSnapshot {
        PodSnapshot {
            name: "er-t-abc-l1-xyz".into(),
            uid: "uid-1".into(),
            node_name: Some("node-1".into()),
            labels: std::collections::BTreeMap::new(),
            annotations: std::collections::BTreeMap::new(),
            phase,
            created_at_unix_seconds: None,
            waiting_reason: None,
            terminated_exit_code: None,
            terminated_reason: None,
            termination_message: None,
            disruption_target: None,
            status_reason: None,
            unschedulable: false,
            scheduling_message: None,
            node_not_ready: false,
        }
    }

    fn running_row() -> RowView {
        RowView {
            state: RowState::Running,
            finish_landed: false,
        }
    }

    fn record_json(state: &str) -> String {
        format!("{{\"v\":1,\"attempt\":\"a\",\"state\":\"{state}\",\"exit\":0,\"snapshot_end\":9}}")
    }

    // ---- pre-start table ----------------------------------------------------

    #[test]
    fn a_fatal_waiting_reason_spends_a_launch_after_its_bound() {
        let mut p = pod(PodPhase::Pending);
        p.waiting_reason = Some("ImagePullBackOff".into());
        assert_eq!(
            pre_start(Some(&p), &job(), None, 59, 0, 0),
            PreStartVerdict::Wait
        );
        assert_eq!(
            pre_start(Some(&p), &job(), None, 60, 0, 0),
            PreStartVerdict::RequeueLaunch {
                reason: "ImagePullBackOff".into(),
                detail: None
            }
        );
    }

    #[test]
    fn unschedulable_is_a_capacity_wait_after_ten_minutes_and_spends_nothing() {
        let mut p = pod(PodPhase::Pending);
        p.unschedulable = true;
        p.scheduling_message = Some("0/2 nodes are available".into());
        assert_eq!(
            pre_start(Some(&p), &job(), None, 0, 599, 0),
            PreStartVerdict::Wait
        );
        let v = pre_start(Some(&p), &job(), None, 0, 600, 0);
        assert_eq!(
            v,
            PreStartVerdict::CapacityWait {
                reason: "Unschedulable".into(),
                detail: Some("0/2 nodes are available".into())
            }
        );
    }

    #[test]
    fn a_quota_failed_create_waits_and_any_other_spends_a_launch() {
        let v = pre_start(
            None,
            &job(),
            Some("pods \"x\" is forbidden: exceeded quota: ..."),
            0,
            0,
            60,
        );
        assert!(matches!(v, PreStartVerdict::CapacityWait { reason, .. } if reason == "quota"));
        let v = pre_start(
            None,
            &job(),
            Some("no PriorityClass named er-task-scheduled"),
            0,
            0,
            60,
        );
        assert!(
            matches!(v, PreStartVerdict::RequeueLaunch { reason, .. } if reason == "FailedCreate")
        );
        let v = pre_start(None, &job(), None, 0, 0, 60);
        assert!(
            matches!(v, PreStartVerdict::RequeueLaunch { reason, .. } if reason == "no_pod_created")
        );
        assert_eq!(
            pre_start(None, &job(), None, 0, 0, 59),
            PreStartVerdict::Wait
        );
    }

    // ---- terminal table -----------------------------------------------------

    #[test]
    fn a_landed_finish_outranks_later_evidence() {
        let mut p = pod(PodPhase::Failed);
        p.terminated_exit_code = Some(137);
        p.terminated_reason = Some("OOMKilled".into());
        let v = terminal(
            RowView {
                state: RowState::Running,
                finish_landed: true,
            },
            Some(&p),
            &job(),
            false,
            0,
        );
        assert_eq!(
            v,
            TerminalVerdict::RecordEvidence {
                exit_code: Some(137),
                reason: Some("OOMKilled".into())
            },
            "evidence annotates; it never revokes a landed report"
        );
    }

    #[test]
    fn a_relaunch_needs_the_pod_provably_gone() {
        // A running pod with a DisruptionTarget is NOT yet actionable: the
        // foreground delete returns at the deletion timestamp and the process
        // may still be writing (clause d).
        let mut p = pod(PodPhase::Running);
        p.disruption_target = Some("EvictionByEvictionAPI".into());
        assert_eq!(
            terminal(running_row(), Some(&p), &job(), false, 0),
            TerminalVerdict::Wait
        );
        // Terminal phase: now it is.
        p.phase = PodPhase::Failed;
        assert_eq!(
            terminal(running_row(), Some(&p), &job(), false, 0),
            TerminalVerdict::Requeue {
                reason: "EvictionByEvictionAPI".into()
            }
        );
    }

    #[test]
    fn a_parsed_record_is_the_verdict() {
        let mut p = pod(PodPhase::Succeeded);
        p.termination_message = Some(record_json("succeeded"));
        let v = terminal(running_row(), Some(&p), &job(), false, 0);
        let TerminalVerdict::ApplyRecord(record) = v else {
            panic!("expected ApplyRecord, got {v:?}");
        };
        assert_eq!(record.state, TerminalState::Succeeded);
        assert_eq!(record.snapshot_end, Some(9));
    }

    #[test]
    fn an_interrupted_record_defers_to_the_platform() {
        let mut p = pod(PodPhase::Failed);
        p.termination_message = Some(record_json("interrupted"));
        // Deadline → timed_out.
        let mut j = job();
        j.failed_deadline = true;
        assert_eq!(
            terminal(running_row(), Some(&p), &j, false, 0),
            TerminalVerdict::TimedOut
        );
        // Disruption → launch budget.
        p.disruption_target = Some("DeletionByTaintManager".into());
        assert_eq!(
            terminal(running_row(), Some(&p), &job(), false, 0),
            TerminalVerdict::Requeue {
                reason: "DeletionByTaintManager".into()
            }
        );
        // Neither → sigterm on the launch budget.
        p.disruption_target = None;
        assert_eq!(
            terminal(running_row(), Some(&p), &job(), false, 0),
            TerminalVerdict::Requeue {
                reason: "sigterm".into()
            }
        );
    }

    #[test]
    fn oom_is_resources_and_exit_137_alone_never_is() {
        let mut p = pod(PodPhase::Failed);
        p.terminated_exit_code = Some(137);
        p.terminated_reason = Some("OOMKilled".into());
        assert!(matches!(
            terminal(running_row(), Some(&p), &job(), false, 0),
            TerminalVerdict::FailResources { reason, .. } if reason == "OOMKilled"
        ));
        // 137 with no reason and no DisruptionTarget: a SIGKILL we cannot
        // attribute — a launch, never an inferred OOM.
        p.terminated_reason = None;
        assert_eq!(
            terminal(running_row(), Some(&p), &job(), false, NO_RECORD_BOUND_SECS),
            TerminalVerdict::Requeue {
                reason: "sigkill".into()
            }
        );
    }

    #[test]
    fn the_kubelets_storage_eviction_is_this_tasks_spill() {
        let mut p = pod(PodPhase::Failed);
        p.status_reason = Some("Evicted".into());
        p.termination_message = Some("emptyDir usage exceeds the limit".into());
        assert!(matches!(
            terminal(running_row(), Some(&p), &job(), false, 0),
            TerminalVerdict::FailResources { reason, .. } if reason == "Evicted"
        ));
        // With a DisruptionTarget it is node-pressure: platform noise, a launch.
        p.disruption_target = Some("TerminationByKubelet".into());
        assert!(matches!(
            terminal(running_row(), Some(&p), &job(), false, 0),
            TerminalVerdict::Requeue { reason } if reason == "TerminationByKubelet"
        ));
    }

    #[test]
    fn no_record_is_never_a_success_and_never_retryable() {
        let p = pod(PodPhase::Failed);
        // Within the bound: wait for the kubelet to surface the message.
        assert_eq!(
            terminal(running_row(), Some(&p), &job(), false, 10),
            TerminalVerdict::Wait
        );
        // Past it: failed/internal for a running row...
        assert_eq!(
            terminal(running_row(), Some(&p), &job(), false, 60),
            TerminalVerdict::FailNoRecord
        );
        // ...and a launch for a queued one (user code never ran).
        assert_eq!(
            terminal(
                RowView {
                    state: RowState::Queued,
                    finish_landed: false
                },
                Some(&p),
                &job(),
                false,
                60
            ),
            TerminalVerdict::Requeue {
                reason: "no_terminal_record".into()
            }
        );
    }

    #[test]
    fn deleted_while_running_without_our_label_is_a_vanish() {
        assert_eq!(
            terminal(running_row(), None, &job(), true, 0),
            TerminalVerdict::Requeue {
                reason: "vanished".into()
            }
        );
        let mut p = pod(PodPhase::Running);
        p.status_reason = Some("NodeLost".into());
        assert_eq!(
            terminal(running_row(), Some(&p), &job(), true, 0),
            TerminalVerdict::Requeue {
                reason: "NodeLost".into()
            }
        );
    }

    #[test]
    fn canceling_and_restarting_rows_take_the_operators_meaning() {
        let mut p = pod(PodPhase::Failed);
        p.terminated_exit_code = Some(3);
        p.terminated_reason = Some("Error".into());
        assert_eq!(
            terminal(
                RowView {
                    state: RowState::Canceling,
                    finish_landed: false
                },
                Some(&p),
                &job(),
                true,
                0
            ),
            TerminalVerdict::SettleCanceled {
                exit_code: Some(3),
                reason: Some("Error".into())
            }
        );
        // Even a real OOM on a restarting row is recorded, never classified:
        // the clear wins.
        p.terminated_reason = Some("OOMKilled".into());
        p.terminated_exit_code = Some(137);
        assert_eq!(
            terminal(
                RowView {
                    state: RowState::Restarting,
                    finish_landed: false
                },
                Some(&p),
                &job(),
                true,
                0
            ),
            TerminalVerdict::CompleteRestart {
                exit_code: Some(137),
                reason: Some("OOMKilled".into())
            }
        );
    }

    #[test]
    fn the_jobs_own_deadline_needs_no_pod() {
        let mut j = job();
        j.failed_deadline = true;
        assert_eq!(
            terminal(running_row(), None, &j, false, 0),
            TerminalVerdict::TimedOut
        );
    }
}
