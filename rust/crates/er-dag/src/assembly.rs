//! Assembly (§3.3/§3.4): the pure, startup-and-test-time validation of a
//! built graph, plus the two fingerprints. A problem here is a refusal to
//! start, not a log line.

use std::collections::BTreeSet;

use sha2::{Digest, Sha256};

use crate::builder::{NodeId, NodeKind};
use crate::policy::{ClassRule, NodePolicy, ResourceClass, TriggerRule};

/// One registered node's compile-time metadata.
#[derive(Debug, Clone)]
pub struct Node {
    pub task_id: &'static str,
    pub version: u32,
    pub kind: NodeKind,
    pub input_names: Vec<&'static str>,
    pub output_name: &'static str,
    pub writes_lake: bool,
    pub trigger_rule: TriggerRule,
    pub teardown: bool,
    pub policy: NodePolicy,
    pub upstream: Vec<NodeId>,
}

/// Why a graph refused to assemble.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum AssemblyError {
    #[error("the graph is empty")]
    Empty,
    #[error("duplicate task name {task_id}")]
    DuplicateTaskName { task_id: &'static str },
    #[error("teardown task {task_id} has data inputs")]
    TeardownWithInputs { task_id: &'static str },
    #[error("task {task_id} escalates on OOM from a fixed L class, which has no next class")]
    EscalateOnFixedL { task_id: &'static str },
    #[error("segment {segment} is not contiguous in topological order")]
    SegmentNotContiguous { segment: &'static str },
    #[error("segment {segment} spans two classes")]
    SegmentClassMismatch { segment: &'static str },
    #[error(
        "lake-writing tasks {a} and {b} have no path between them: two concurrent writers of one tenant lake"
    )]
    UnorderedLakeWriters { a: &'static str, b: &'static str },
}

/// An assembled, validated DAG. Node index == topological `seq` (handles only
/// reference earlier nodes, so registration order is already topological).
#[derive(Debug, Clone)]
pub struct Dag {
    pub name: String,
    pub nodes: Vec<Node>,
    pub structural_fingerprint: String,
    pub policy_hash: String,
}

impl Dag {
    /// The node registered under a task name, with its index.
    #[must_use]
    pub fn node_by_task_id(&self, task_id: &str) -> Option<(usize, &Node)> {
        self.nodes
            .iter()
            .enumerate()
            .find(|(_, n)| n.task_id == task_id)
    }

    /// Direct downstream indices of `idx`.
    #[must_use]
    pub fn downstream_of(&self, idx: usize) -> Vec<usize> {
        self.nodes
            .iter()
            .enumerate()
            .filter(|(_, n)| n.upstream.iter().any(|u| u.index() == idx))
            .map(|(i, _)| i)
            .collect()
    }

    /// Is `to` reachable from `from` along edges?
    #[must_use]
    pub fn reaches(&self, from: usize, to: usize) -> bool {
        if from == to {
            return true;
        }
        let mut frontier = vec![from];
        let mut seen = BTreeSet::new();
        while let Some(i) = frontier.pop() {
            for d in self.downstream_of(i) {
                if d == to {
                    return true;
                }
                if seen.insert(d) {
                    frontier.push(d);
                }
            }
        }
        false
    }

    /// The per-node structural description a fingerprint diff names (§3.4).
    #[must_use]
    pub fn structural_lines(&self) -> Vec<String> {
        self.nodes
            .iter()
            .map(|n| {
                format!(
                    "{}|v{}|{}|in:{}|out:{}|lake:{}|rule:{}|teardown:{}|up:{}",
                    n.task_id,
                    n.version,
                    n.kind.as_str(),
                    n.input_names.join(","),
                    n.output_name,
                    n.writes_lake,
                    n.trigger_rule.as_str(),
                    n.teardown,
                    n.upstream
                        .iter()
                        .map(|u| u.index().to_string())
                        .collect::<Vec<_>>()
                        .join(","),
                )
            })
            .collect()
    }

    fn policy_lines(&self) -> Vec<String> {
        self.nodes
            .iter()
            .map(|n| {
                let class = match n.policy.class {
                    ClassRule::Fixed(c) => format!("fixed:{}", c.as_str()),
                    ClassRule::FromRun => "from_run".to_string(),
                };
                format!(
                    "{}|retries:{}|backoff:{}/{}|timeout:{:?}|class:{class}|escalate:{}|segment:{:?}|threads:{:?}",
                    n.task_id,
                    n.policy.retries,
                    n.policy.backoff.base_secs,
                    n.policy.backoff.cap_secs,
                    n.policy.timeout_secs,
                    n.policy.escalate_on_oom,
                    n.policy.segment,
                    n.policy.threads,
                )
            })
            .collect()
    }
}

fn sha256_hex(parts: &[String]) -> String {
    let mut hasher = Sha256::new();
    for p in parts {
        hasher.update(p.as_bytes());
        hasher.update(b"\n");
    }
    let digest = hasher.finalize();
    let mut out = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// Run the §3.3 checks and compute both hashes.
///
/// # Errors
/// The graph breaks an assembly invariant.
pub fn assemble(name: String, nodes: Vec<Node>) -> Result<Dag, AssemblyError> {
    if nodes.is_empty() {
        return Err(AssemblyError::Empty);
    }

    let mut seen = BTreeSet::new();
    for n in &nodes {
        if !seen.insert(n.task_id) {
            return Err(AssemblyError::DuplicateTaskName { task_id: n.task_id });
        }
        if n.teardown && !n.input_names.is_empty() {
            return Err(AssemblyError::TeardownWithInputs { task_id: n.task_id });
        }
        if n.policy.escalate_on_oom && n.policy.class == ClassRule::Fixed(ResourceClass::L) {
            return Err(AssemblyError::EscalateOnFixedL { task_id: n.task_id });
        }
    }

    // Segments: contiguous in topological order, one class rule.
    let segments: BTreeSet<&'static str> = nodes.iter().filter_map(|n| n.policy.segment).collect();
    for segment in segments {
        let members: Vec<usize> = nodes
            .iter()
            .enumerate()
            .filter(|(_, n)| n.policy.segment == Some(segment))
            .map(|(i, _)| i)
            .collect();
        let contiguous = members.windows(2).all(|w| w[1] == w[0] + 1);
        if !contiguous {
            return Err(AssemblyError::SegmentNotContiguous { segment });
        }
        let first_class = nodes[members[0]].policy.class;
        if members
            .iter()
            .any(|&i| nodes[i].policy.class != first_class)
        {
            return Err(AssemblyError::SegmentClassMismatch { segment });
        }
    }

    let dag = Dag {
        name,
        nodes,
        structural_fingerprint: String::new(),
        policy_hash: String::new(),
    };

    // Two lake-writing tasks with no path between them would be two concurrent
    // writers of one tenant lake — the graph-level image of
    // `jobs_one_active_per_org`; the DB-level image is
    // `task_instances_one_pod_per_org` (§3.3).
    let writers: Vec<usize> = dag
        .nodes
        .iter()
        .enumerate()
        .filter(|(_, n)| n.writes_lake)
        .map(|(i, _)| i)
        .collect();
    for (pos, &a) in writers.iter().enumerate() {
        for &b in &writers[pos + 1..] {
            if !dag.reaches(a, b) && !dag.reaches(b, a) {
                return Err(AssemblyError::UnorderedLakeWriters {
                    a: dag.nodes[a].task_id,
                    b: dag.nodes[b].task_id,
                });
            }
        }
    }

    let mut structural = vec![format!("dag:{}", dag.name)];
    structural.extend(dag.structural_lines());
    let structural_fingerprint = sha256_hex(&structural);
    let policy_hash = sha256_hex(&dag.policy_lines());

    Ok(Dag {
        structural_fingerprint,
        policy_hash,
        ..dag
    })
}
