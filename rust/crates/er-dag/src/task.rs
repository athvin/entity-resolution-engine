//! The task traits and the durable edge types (§3.2). A task body receives a
//! context the leaf binary fills and a tuple of rehydrated upstream outputs;
//! everything that crosses a pod boundary is a *reference* (a [`LakeRef`] or a
//! [`Plan`]), never a value.

use std::marker::PhantomData;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::builder::{Handle, NodeId};
use crate::policy::NodePolicy;

/// A problem with a durable reference: a document that is not the expected
/// shape, or a probe that could not be answered.
#[derive(Debug, thiserror::Error)]
pub enum RefError {
    #[error("malformed {expected} reference")]
    Malformed { expected: &'static str },
    #[error("serializing {what} reference")]
    Serialize {
        what: &'static str,
        #[source]
        source: serde_json::Error,
    },
    #[error("probing lake snapshot {snapshot}")]
    Probe {
        snapshot: i64,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}

/// Does a durable reference still exist? `Missing` once `lake_maintain`'s
/// `expire_snapshots` removed the snapshot it pins. There is no `Changed`: a
/// snapshot is immutable (§3.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReferenceExistence {
    Present,
    Missing,
}

/// Supplied by the leaf binary, which owns the lake session; this crate never
/// names the engine. The one question a `LakeRef` probe asks is DuckLake
/// catalog introspection — does the snapshot still exist — never a relation
/// read.
pub trait Prober {
    /// # Errors
    /// The underlying catalog query failed.
    fn snapshot_exists(&self, snapshot: i64) -> Result<bool, RefError>;
}

/// A durable, addressable output: what a task may hand downstream. The only
/// implementations are [`LakeRef`] (a set of lake relations at a committed
/// snapshot) and [`Plan`] (a small typed document stored on the producer's
/// row).
pub trait DurableOutput: Sized + 'static {
    const STABLE_NAME: &'static str;

    /// # Errors
    /// The reference could not be serialized (a non-JSON-representable plan).
    fn serialize_reference(&self) -> Result<serde_json::Value, RefError>;

    /// # Errors
    /// The stored document is not this reference's shape.
    fn rehydrate(v: &serde_json::Value) -> Result<Self, RefError>;

    /// Existence probe, used by the clear handler's demand resolution and the
    /// pod's input check.
    ///
    /// # Errors
    /// The probe itself failed (distinct from a clean `Missing`).
    fn probe(&self, prober: &dyn Prober) -> Result<ReferenceExistence, RefError>;
}

/// A named set of lake relations an edge carries: the vocabulary of lake
/// hand-offs. Every relation name must be in `er_schema::model::all_specs()` —
/// the leaf binary's assembly test enforces it (§3.2).
pub trait LakeEdge: 'static {
    const STABLE_NAME: &'static str;
    const RELATIONS: &'static [&'static str];
}

/// A set of lake relations at the DuckLake snapshot the producer committed.
/// The only lake edge.
pub struct LakeRef<E: LakeEdge> {
    pub snapshot_end: i64,
    pub rows: i64,
    _e: PhantomData<fn() -> E>,
}

// Manual impls: a derive would demand the marker type `E` itself be
// Debug/PartialEq/Copy, which a unit edge type never is.
impl<E: LakeEdge> std::fmt::Debug for LakeRef<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LakeRef")
            .field("edge", &E::STABLE_NAME)
            .field("snapshot_end", &self.snapshot_end)
            .field("rows", &self.rows)
            .finish()
    }
}
impl<E: LakeEdge> Clone for LakeRef<E> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<E: LakeEdge> Copy for LakeRef<E> {}
impl<E: LakeEdge> PartialEq for LakeRef<E> {
    fn eq(&self, other: &Self) -> bool {
        self.snapshot_end == other.snapshot_end && self.rows == other.rows
    }
}
impl<E: LakeEdge> Eq for LakeRef<E> {}

impl<E: LakeEdge> LakeRef<E> {
    #[must_use]
    pub fn new(snapshot_end: i64, rows: i64) -> Self {
        LakeRef {
            snapshot_end,
            rows,
            _e: PhantomData,
        }
    }
}

impl<E: LakeEdge> DurableOutput for LakeRef<E> {
    const STABLE_NAME: &'static str = E::STABLE_NAME;

    fn serialize_reference(&self) -> Result<serde_json::Value, RefError> {
        Ok(serde_json::json!({
            "edge": E::STABLE_NAME,
            "snapshot_end": self.snapshot_end,
            "rows": self.rows,
        }))
    }

    fn rehydrate(v: &serde_json::Value) -> Result<Self, RefError> {
        let malformed = || RefError::Malformed {
            expected: E::STABLE_NAME,
        };
        if v.get("edge").and_then(serde_json::Value::as_str) != Some(E::STABLE_NAME) {
            return Err(malformed());
        }
        let snapshot_end = v
            .get("snapshot_end")
            .and_then(serde_json::Value::as_i64)
            .ok_or_else(malformed)?;
        let rows = v
            .get("rows")
            .and_then(serde_json::Value::as_i64)
            .ok_or_else(malformed)?;
        Ok(LakeRef::new(snapshot_end, rows))
    }

    fn probe(&self, prober: &dyn Prober) -> Result<ReferenceExistence, RefError> {
        Ok(if prober.snapshot_exists(self.snapshot_end)? {
            ReferenceExistence::Present
        } else {
            ReferenceExistence::Missing
        })
    }
}

/// The typed document behind a [`Plan`] edge; the leaf binary declares one per
/// plan shape (`RunPlan`, `DrainedActions`, …).
pub trait PlanDoc: Serialize + DeserializeOwned + 'static {
    const STABLE_NAME: &'static str;
}

/// A small typed document the control plane stores on the producer's row
/// (`task_instances.output`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan<P: PlanDoc>(pub P);

impl<P: PlanDoc> DurableOutput for Plan<P> {
    const STABLE_NAME: &'static str = P::STABLE_NAME;

    fn serialize_reference(&self) -> Result<serde_json::Value, RefError> {
        let doc = serde_json::to_value(&self.0).map_err(|source| RefError::Serialize {
            what: P::STABLE_NAME,
            source,
        })?;
        Ok(serde_json::json!({ "plan": P::STABLE_NAME, "doc": doc }))
    }

    fn rehydrate(v: &serde_json::Value) -> Result<Self, RefError> {
        let malformed = || RefError::Malformed {
            expected: P::STABLE_NAME,
        };
        if v.get("plan").and_then(serde_json::Value::as_str) != Some(P::STABLE_NAME) {
            return Err(malformed());
        }
        let doc = v.get("doc").ok_or_else(malformed)?;
        let parsed: P = serde_json::from_value(doc.clone()).map_err(|_| malformed())?;
        Ok(Plan(parsed))
    }

    fn probe(&self, _prober: &dyn Prober) -> Result<ReferenceExistence, RefError> {
        // A plan lives on the producer's task_instances row; if the row was
        // readable enough to rehydrate from, the reference exists.
        Ok(ReferenceExistence::Present)
    }
}

/// Marker for outputs a control task may produce: a [`Plan`] only. A control
/// task with a `LakeRef` output does not type-check — the structural teeth
/// behind "a control task cannot run a stage" (§3.6).
pub trait PlanOutput: DurableOutput {}
impl<P: PlanDoc> PlanOutput for Plan<P> {}

mod sealed {
    pub trait Sealed {}
}

/// Positional inputs: `()` or a tuple of [`DurableOutput`]s, up to 8. Sealed —
/// the arity table below is the whole vocabulary.
pub trait DepList: sealed::Sealed + Sized + 'static {
    /// The handle tuple the builder takes: one `Handle<_>` per input, in order.
    type Handles: Copy;
    const ARITY: usize;
    fn ids(handles: &Self::Handles) -> Vec<NodeId>;
    fn stable_names() -> Vec<&'static str>;
}

impl sealed::Sealed for () {}
impl DepList for () {
    type Handles = ();
    const ARITY: usize = 0;
    fn ids((): &Self::Handles) -> Vec<NodeId> {
        Vec::new()
    }
    fn stable_names() -> Vec<&'static str> {
        Vec::new()
    }
}

macro_rules! deps {
    ($arity:literal, $( $t:ident : $idx:tt ),+) => {
        impl<$( $t: DurableOutput ),+> sealed::Sealed for ($( $t, )+) {}
        impl<$( $t: DurableOutput ),+> DepList for ($( $t, )+) {
            type Handles = ($( Handle<$t>, )+);
            const ARITY: usize = $arity;
            fn ids(handles: &Self::Handles) -> Vec<NodeId> {
                vec![$( handles.$idx.id() ),+]
            }
            fn stable_names() -> Vec<&'static str> {
                vec![$( $t::STABLE_NAME ),+]
            }
        }
    };
}

deps!(1, A: 0);
deps!(2, A: 0, B: 1);
deps!(3, A: 0, B: 1, C: 2);
deps!(4, A: 0, B: 1, C: 2, D: 3);
deps!(5, A: 0, B: 1, C: 2, D: 3, E: 4);
deps!(6, A: 0, B: 1, C: 2, D: 3, E: 4, F: 5);
deps!(7, A: 0, B: 1, C: 2, D: 3, E: 4, F: 5, G: 6);
deps!(8, A: 0, B: 1, C: 2, D: 3, E: 4, F: 5, G: 6, H: 7);

/// Progress counters as the task reports them; replaced on every heartbeat
/// and at finish.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Counters(pub serde_json::Value);

/// A task's result. `NoOp` is exit 10: a success with `outcome = no_op`
/// (`ExitCode::NothingToDo`).
pub enum Outcome<O> {
    Done(O, Counters),
    NoOp(O),
}

/// A task's failure. The class comes from [`er_errors::ErError`]; `Skip` is
/// the task branching (never the graph — §3.3).
pub enum TaskError {
    Failed(er_errors::ErError),
    Skip,
}

/// The identity and inputs of one try, as the work-order channel delivers it.
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct WorkOrder {
    pub org_id: String,
    pub job_id: String,
    /// `jobs.id == run_id` is load-bearing (§4.1).
    pub run_id: String,
    pub task_id: String,
    pub try_number: i64,
    pub resource_class: Option<String>,
    pub segment: Option<String>,
    /// The positional upstream references, serialized.
    pub input: serde_json::Value,
    /// The job's kind-specific parameters.
    pub params: serde_json::Value,
    /// The tenant's lake env (`ER_LAKE_CATALOG`, …) from `orgs.env`.
    pub lake_env: serde_json::Value,
}

/// Where a running task leaves its latest counter snapshot for the heartbeat
/// thread to pick up.
#[derive(Debug, Default)]
pub struct CounterSink {
    latest: Mutex<Option<serde_json::Value>>,
}

impl CounterSink {
    pub fn set(&self, v: serde_json::Value) {
        *self.latest.lock().expect("counter sink lock") = Some(v);
    }

    #[must_use]
    pub fn snapshot(&self) -> Option<serde_json::Value> {
        self.latest.lock().expect("counter sink lock").clone()
    }
}

/// A cooperative cancellation flag the SIGTERM thread sets; the body may poll
/// it between its own statements (§5.6). The engine-level interrupt (DuckDB's
/// `interrupt_handle`) is the leaf binary's to wire in Phase 2.
#[derive(Debug, Default)]
pub struct CancelToken {
    flag: AtomicBool,
}

impl CancelToken {
    pub fn cancel(&self) {
        self.flag.store(true, Ordering::SeqCst);
    }

    #[must_use]
    pub fn is_canceled(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }
}

/// What a pod task body receives. This crate defines it; the leaf binary's
/// verb fills it (`S` is the lake session type — `dedupe_core::Session` there,
/// never named here).
pub struct TaskCtx<'a, S> {
    pub order: &'a WorkOrder,
    pub session: &'a S,
    pub counters: &'a CounterSink,
    /// Set by the heartbeat thread on a zero-row heartbeat; checked before the
    /// last write-back (§6.3 step 6).
    pub fence: &'a AtomicBool,
    pub cancel: &'a CancelToken,
}

/// What a control task body receives (`C` is the control-plane client type).
pub struct ControlCtx<'a, C> {
    pub order: &'a WorkOrder,
    pub client: &'a C,
}

/// A pod task: a pipeline stage that runs in a Kubernetes Job and owns a lake
/// session (§3.2).
pub trait Task<S>: Default + 'static {
    /// Author-declared stable identity, recorded on the row and the pod. A
    /// Rust rename does not move it.
    const NAME: &'static str;
    /// Feeds the structural fingerprint; bumped when the task's declared
    /// statement set changes (§9.4).
    const VERSION: u32;
    /// Positional inputs: `()` | `(A,)` | `(A, B)` … up to 8, each a
    /// [`DurableOutput`]. Sealed.
    type Input: DepList;
    /// Always a reference, never a value: an in-memory edge does not
    /// type-check.
    type Output: DurableOutput;
    /// True for every task that writes the tenant lake. Feeds the assembly
    /// single-writer check.
    const WRITES_LAKE: bool;

    #[must_use]
    fn policy() -> NodePolicy {
        NodePolicy::default()
    }

    /// Runs inside the pod. `&mut self` is what makes "try n+1 never overlaps
    /// try n" free.
    ///
    /// # Errors
    /// A classified stage failure, or `Skip`.
    fn run(
        &mut self,
        ctx: &mut TaskCtx<'_, S>,
        input: Self::Input,
    ) -> Result<Outcome<Self::Output>, TaskError>;
}

/// A control task (§3.6): the Postgres half of a kind, run inside the
/// dispatcher with the control plane's own credentials, under the same ledger
/// discipline as a pod task — and structurally unable to run a stage.
pub trait ControlTask<C>: Default + 'static {
    const NAME: &'static str;
    const VERSION: u32;
    type Input: DepList;
    /// A [`Plan`] only: a `LakeRef` output on a control task does not compile.
    type Output: PlanOutput;

    #[must_use]
    fn policy() -> NodePolicy {
        NodePolicy::control()
    }

    /// # Errors
    /// A classified failure, or `Skip`.
    fn run(
        &mut self,
        ctx: &mut ControlCtx<'_, C>,
        input: Self::Input,
    ) -> Result<Outcome<Self::Output>, TaskError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestEdge;
    impl LakeEdge for TestEdge {
        const STABLE_NAME: &'static str = "test_edge";
        const RELATIONS: &'static [&'static str] = &["runs"];
    }

    #[derive(Serialize, serde::Deserialize, PartialEq, Debug)]
    struct TestPlan {
        n: i64,
    }
    impl PlanDoc for TestPlan {
        const STABLE_NAME: &'static str = "test_plan";
    }

    struct FixedProber(bool);
    impl Prober for FixedProber {
        fn snapshot_exists(&self, _snapshot: i64) -> Result<bool, RefError> {
            Ok(self.0)
        }
    }

    #[test]
    fn lake_ref_round_trips_and_probes() {
        let r = LakeRef::<TestEdge>::new(41, 10);
        let v = r.serialize_reference().unwrap();
        let back = LakeRef::<TestEdge>::rehydrate(&v).unwrap();
        assert_eq!(back, r);
        assert!(matches!(
            r.probe(&FixedProber(true)).unwrap(),
            ReferenceExistence::Present
        ));
        assert!(matches!(
            r.probe(&FixedProber(false)).unwrap(),
            ReferenceExistence::Missing
        ));
        // A document for a different edge refuses to rehydrate.
        let foreign = serde_json::json!({"edge": "other", "snapshot_end": 1, "rows": 1});
        assert!(LakeRef::<TestEdge>::rehydrate(&foreign).is_err());
    }

    #[test]
    fn plan_round_trips_and_is_always_present() {
        let p = Plan(TestPlan { n: 7 });
        let v = p.serialize_reference().unwrap();
        let back = Plan::<TestPlan>::rehydrate(&v).unwrap();
        assert_eq!(back.0, TestPlan { n: 7 });
        assert!(matches!(
            back.probe(&FixedProber(false)).unwrap(),
            ReferenceExistence::Present
        ));
    }
}
