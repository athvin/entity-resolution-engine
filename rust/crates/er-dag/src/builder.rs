//! The typed builder (§3.2/§3.3). Handles are minted only here, so an edge can
//! only point at an already-registered node — a cycle is unwritable, and
//! registration order is therefore already a topological order.

use std::marker::PhantomData;

use crate::assembly::{AssemblyError, Dag, Node, assemble};
use crate::policy::TriggerRule;
use crate::task::{ControlTask, DepList, DurableOutput, Task};

/// A node's position in its DAG (also its topological `seq`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeId(pub(crate) usize);

impl NodeId {
    #[must_use]
    pub fn index(self) -> usize {
        self.0
    }
}

/// A typed reference to a registered node's output. Only the builder mints
/// these.
pub struct Handle<O: DurableOutput> {
    id: NodeId,
    _o: PhantomData<fn() -> O>,
}

impl<O: DurableOutput> Clone for Handle<O> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<O: DurableOutput> Copy for Handle<O> {}

impl<O: DurableOutput> Handle<O> {
    #[must_use]
    pub fn id(self) -> NodeId {
        self.id
    }
}

/// Where a node's body runs: a Kubernetes Job, or the dispatcher process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    Pod,
    Control,
}

impl NodeKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            NodeKind::Pod => "pod",
            NodeKind::Control => "control",
        }
    }
}

/// Builds one kind's DAG. `S` is the lake session type a [`Task`] body takes,
/// `C` the control-plane client a [`ControlTask`] body takes; the leaf binary
/// fixes both, and this crate never names an engine or driver type.
pub struct DagBuilder<S, C> {
    name: String,
    nodes: Vec<Node>,
    _s: PhantomData<fn() -> S>,
    _c: PhantomData<fn() -> C>,
}

impl<S, C> DagBuilder<S, C> {
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        DagBuilder {
            name: name.into(),
            nodes: Vec::new(),
            _s: PhantomData,
            _c: PhantomData,
        }
    }

    fn push(&mut self, node: Node) -> NodeId {
        let id = NodeId(self.nodes.len());
        self.nodes.push(node);
        id
    }

    /// Register a source task (no inputs).
    pub fn source<T: Task<S, Input = ()>>(&mut self) -> Handle<T::Output> {
        self.then::<T>(())
    }

    /// Register a pod task whose inputs are the given handles, positionally.
    /// The tuple must match `T::Input`; a mismatch names both types in the
    /// compile error.
    pub fn then<T: Task<S>>(&mut self, deps: <T::Input as DepList>::Handles) -> Handle<T::Output> {
        let policy = T::policy();
        let id = self.push(Node {
            task_id: T::NAME,
            version: T::VERSION,
            kind: NodeKind::Pod,
            input_names: <T::Input as DepList>::stable_names(),
            output_name: <T::Output as DurableOutput>::STABLE_NAME,
            writes_lake: T::WRITES_LAKE,
            trigger_rule: TriggerRule::AllSucceeded,
            teardown: policy.teardown,
            policy,
            upstream: <T::Input as DepList>::ids(&deps),
        });
        Handle {
            id,
            _o: PhantomData,
        }
    }

    /// Register a control task (§3.6): dispatcher-local, no org slot, no class,
    /// and — by the `PlanOutput` bound — structurally unable to emit a lake
    /// edge.
    pub fn control<T: ControlTask<C>>(
        &mut self,
        deps: <T::Input as DepList>::Handles,
    ) -> Handle<T::Output> {
        let policy = T::policy();
        let id = self.push(Node {
            task_id: T::NAME,
            version: T::VERSION,
            kind: NodeKind::Control,
            input_names: <T::Input as DepList>::stable_names(),
            output_name: <T::Output as DurableOutput>::STABLE_NAME,
            writes_lake: false,
            trigger_rule: TriggerRule::AllSucceeded,
            teardown: policy.teardown,
            policy,
            upstream: <T::Input as DepList>::ids(&deps),
        });
        Handle {
            id,
            _o: PhantomData,
        }
    }

    /// Register a data-independent epilogue (`finish`): `T: Task<Input = ()>`
    /// is the typestate that makes a non-default trigger rule on a
    /// data-dependent node a type error (§3.3). `after` is its control edge
    /// set.
    pub fn epilogue<T: Task<S, Input = ()>>(
        &mut self,
        after: &[NodeId],
        rule: TriggerRule,
    ) -> Handle<T::Output> {
        let policy = T::policy();
        let id = self.push(Node {
            task_id: T::NAME,
            version: T::VERSION,
            kind: NodeKind::Pod,
            input_names: Vec::new(),
            output_name: <T::Output as DurableOutput>::STABLE_NAME,
            writes_lake: T::WRITES_LAKE,
            trigger_rule: rule,
            teardown: policy.teardown,
            policy,
            upstream: after.to_vec(),
        });
        Handle {
            id,
            _o: PhantomData,
        }
    }

    /// Assemble: run every §3.3 check and compute both fingerprints and the
    /// topological order.
    ///
    /// # Errors
    /// The graph breaks an assembly invariant.
    pub fn build(self) -> Result<Dag, AssemblyError> {
        assemble(self.name, self.nodes)
    }
}
