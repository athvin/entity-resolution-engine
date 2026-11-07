//! The Kubernetes half of the batch-orchestration design (§6, carried by the
//! `er-engine-ops` skill): the
//! [`PodApi`] seam the dispatcher launches and watches through (one production
//! implementation over `kube`, one scripted test double in er-server's tests),
//! the per-attempt Job manifest rendering, the watch discipline, and the pure
//! pre-start and terminal classifiers of §6.5 — every row of those tables is a
//! unit test here.
//!
//! The crate deliberately holds no Postgres and no engine: it turns platform
//! evidence into verdicts; er-server's dispatcher turns verdicts into
//! statements.

pub mod api;
pub mod classify;
pub mod manifest;
pub mod observer;
pub mod record;

pub use api::{
    ApiError, EventSnapshot, JobSnapshot, KubeApi, Listing, ObjectKind, PodApi, PodPhase,
    PodSnapshot, Selector, WatchFrame,
};
pub use classify::{
    FATAL_WAIT_BOUND_SECS, NO_POD_BOUND_SECS, NO_RECORD_BOUND_SECS, PreStartVerdict, RowState,
    RowView, TerminalVerdict, UNSCHEDULABLE_BOUND_SECS, pre_start, terminal,
};

/// Re-exported so the dispatcher can hold typed manifests without its own
/// k8s-openapi dependency edge.
pub use k8s_openapi;
pub use manifest::{RenderAttempt, RenderError, job_name, render_job};
pub use record::{TerminalRecord, TerminalState};
