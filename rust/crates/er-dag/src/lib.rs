//! The compiled-DAG library of the batch-orchestration design, carried by the
//! `er-engine-ops` skill as `references/batch-orchestration-design.md`.
//!
//! A **task is a pipeline stage**; a **DAG is a job kind**, written in Rust in
//! the one multicall binary, with typed durable edges so that wiring, arity and
//! trigger-rule misuse fail at `cargo build`, and invalid graph edges fail at
//! assembly:
//!
//! - Handles are minted only by the builder, and assembly rejects foreign or
//!   non-topological edges before readiness evaluates the graph.
//! - `then::<T>(deps)` requires the handle tuple to match `T::Input`
//!   positionally; a mismatch names both types in the compile error.
//! - `Output: DurableOutput` means an in-memory type cannot be an edge.
//! - `epilogue::<T>` is bounded `T: Task<Input = ()>`, so a non-default
//!   trigger rule on a data-dependent node is a type error.
//! - A control task's `Output: PlanOutput`, so a `LakeRef` edge out of a
//!   control task does not type-check.
//!
//! The crate deliberately knows nothing about Kubernetes, Postgres or the
//! engine: the two session parameters (`S` on [`Task`], `C` on
//! [`ControlTask`]) are fixed by the leaf binary, and every durable value
//! crosses as `serde_json::Value`. The state of record lives in Postgres
//! (the `task_instance` entity); this crate only computes — assembly,
//! fingerprints, readiness verdicts and the clear closure are pure functions.

pub mod assembly;
pub mod builder;
pub mod policy;
pub mod readiness;
pub mod resume;
pub mod task;
#[cfg(test)]
mod tests;

pub use assembly::{AssemblyError, Dag, Node};
pub use builder::{DagBuilder, Handle, NodeId, NodeKind};
pub use policy::{Backoff, ClassRule, NodePolicy, ResourceClass, TriggerRule};
pub use readiness::{
    PassAction, SettleVerdict, StateClass, TaskState, Verdict, evaluate, pass, settle,
};
pub use resume::{ClearPlan, clear_closure};
pub use task::{
    CancelToken, ControlCtx, ControlTask, CounterSink, Counters, DepList, DurableOutput, LakeEdge,
    LakeRef, Outcome, Plan, PlanDoc, PlanOutput, Prober, RefError, ReferenceExistence, Task,
    TaskCtx, TaskError, WorkOrder,
};
