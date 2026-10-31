//! The clear closure (§5.4): dagr's demand-driven resume with the operator's
//! selection as the seed. Close downward (everything reachable re-runs), then
//! resolve demand upward: each re-running task demands its inputs; an upstream
//! whose durable output still exists is rehydrated from its row, one whose
//! snapshot `lake_maintain` has expired joins the must-run set — recursively,
//! because a drafted-in upstream demands its own inputs too.

use std::collections::BTreeSet;

use crate::assembly::Dag;
use crate::task::ReferenceExistence;

/// The clear's computed scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClearPlan {
    /// Node indices that re-run: the seeds, their downstream closure, and
    /// every upstream drafted in by a missing reference.
    pub must_run: BTreeSet<usize>,
    /// The subset of `must_run` added by upward demand (shown expanded in the
    /// confirm dialog, never refused with a bare 409).
    pub demanded_upstream: BTreeSet<usize>,
}

/// Compute the closure. `probe(idx)` answers whether node `idx`'s recorded
/// output still exists (`Missing` once its snapshot was expired).
pub fn clear_closure(
    dag: &Dag,
    seeds: &[usize],
    include_downstream: bool,
    probe: impl Fn(usize) -> ReferenceExistence,
) -> ClearPlan {
    let mut must_run: BTreeSet<usize> = seeds.iter().copied().collect();

    if include_downstream {
        let mut frontier: Vec<usize> = must_run.iter().copied().collect();
        while let Some(i) = frontier.pop() {
            for d in dag.downstream_of(i) {
                if must_run.insert(d) {
                    frontier.push(d);
                }
            }
        }
    }

    // Upward demand, to fixpoint: a must-run node demands each upstream whose
    // output no longer exists.
    let mut demanded_upstream = BTreeSet::new();
    let mut frontier: Vec<usize> = must_run.iter().copied().collect();
    while let Some(i) = frontier.pop() {
        for u in &dag.nodes[i].upstream {
            let u = u.index();
            if must_run.contains(&u) {
                continue;
            }
            if probe(u) == ReferenceExistence::Missing {
                must_run.insert(u);
                demanded_upstream.insert(u);
                frontier.push(u);
            }
        }
    }

    ClearPlan {
        must_run,
        demanded_upstream,
    }
}
