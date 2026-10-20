//! The S4.4.2 never-cut fixpoint (port of the PURE core of
//! `src/er/review/never_cut.py::never_cut_fixpoint`).
//!
//! Cut the minimum-probability unprotected edge on the shortest path between each
//! active `never` pair that is currently co-clustered, until none is co-clustered
//! or a pair's every path is fully protected (escalate). Cutting is monotone, so a
//! bounded loop either settles or is a protected-edge cycle / defect
//! ([`NeverCutError::NonConvergence`]). PURE — no connection, no clock, no write.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use er_ids::canonicalize_pair;

use crate::assertions::{Assertion, AssertionKind};

/// `(rec_a_key, rec_b_key, match_probability)` — one clustering edge.
pub type PathEdge = (String, String, f64);

/// One edge the run decided to cut, and the assertion that required it.
#[derive(Debug, Clone, PartialEq)]
pub struct Cut {
    pub rec_a_key: String,
    pub rec_b_key: String,
    pub match_probability: f64,
    pub assertion_id: String,
}

/// What the fixpoint decided.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CutResult {
    pub cuts: Vec<Cut>,
    /// `never` pairs every path between which was fully protected: `(a, b, assertion_id)`.
    pub escalations: Vec<(String, String, String)>,
    pub iterations: usize,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum NeverCutError {
    #[error(
        "the S4.4.2 never-cut loop did not settle in {max_iterations} round(s); {remaining} pair(s) remain co-clustered — a protected-edge cycle or a defect, not slow convergence"
    )]
    NonConvergence {
        max_iterations: usize,
        remaining: usize,
    },
    #[error("the S4.4.2 never-cut loop made no progress on {0} co-clustered never pair(s)")]
    NoProgress(usize),
}

fn canon(a: &str, b: &str) -> (String, String) {
    canonicalize_pair(a, b).expect("distinct endpoints")
}

/// Undirected adjacency with every neighbour list sorted (the lexicographic path
/// tiebreak is only well defined with a fixed visitation order).
fn adjacency(edges: &[PathEdge]) -> BTreeMap<String, Vec<String>> {
    let mut graph: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (left, right, _) in edges {
        graph.entry(left.clone()).or_default().insert(right.clone());
        graph.entry(right.clone()).or_default().insert(left.clone());
    }
    graph
        .into_iter()
        .map(|(node, neighbours)| (node, neighbours.into_iter().collect()))
        .collect()
}

/// The S4.4.2 path: fewest hops, then the lexicographically smallest vertex
/// sequence. BFS settles hop-count; each node keeps the best sequence at its own
/// depth, replaced by a same-depth predecessor whose sequence sorts smaller.
#[must_use]
pub fn shortest_path(source: &str, target: &str, edges: &[PathEdge]) -> Option<Vec<String>> {
    if source == target {
        return Some(vec![source.to_string()]);
    }
    let graph = adjacency(edges);
    if !graph.contains_key(source) || !graph.contains_key(target) {
        return None;
    }
    let mut best: BTreeMap<String, Vec<String>> = BTreeMap::new();
    best.insert(source.to_string(), vec![source.to_string()]);
    let mut depth: BTreeMap<String, usize> = BTreeMap::new();
    depth.insert(source.to_string(), 0);
    let mut frontier: VecDeque<String> = VecDeque::new();
    frontier.push_back(source.to_string());

    while let Some(node) = frontier.pop_front() {
        let node_depth = depth[&node];
        let node_best = best[&node].clone();
        for neighbour in &graph[&node] {
            let mut candidate = node_best.clone();
            candidate.push(neighbour.clone());
            match depth.get(neighbour) {
                None => {
                    depth.insert(neighbour.clone(), node_depth + 1);
                    best.insert(neighbour.clone(), candidate);
                    frontier.push_back(neighbour.clone());
                }
                Some(&d) if d == node_depth + 1 && candidate < best[neighbour] => {
                    best.insert(neighbour.clone(), candidate);
                }
                _ => {}
            }
        }
    }
    best.get(target).cloned()
}

/// The minimum-probability cuttable edge on `path`, tiebroken by
/// `(probability ASC, rec_a_key ASC, rec_b_key ASC)`; `None` when every hop is
/// uncuttable (the escalation condition). An edge is uncuttable when its
/// probability is `>= cut_protect_probability` (an `always`/high-confidence edge)
/// OR either endpoint is a protected record (PR 1.3c): never-cut must never sever
/// a protected record's membership, so such an edge is off-limits exactly like a
/// protected-probability one, and a pair reachable only through protected records
/// escalates instead of cutting. `protected` is empty on the base path, so this is
/// byte-identical to the pre-1.3c behaviour.
#[must_use]
pub fn choose_cut_edge(
    path: &[String],
    edges: &[PathEdge],
    cut_protect_probability: f64,
    protected: &BTreeSet<String>,
) -> Option<PathEdge> {
    let probability: BTreeMap<(String, String), f64> =
        edges.iter().map(|(l, r, v)| (canon(l, r), *v)).collect();
    let mut candidates: Vec<PathEdge> = Vec::new();
    for window in path.windows(2) {
        let pair = canon(&window[0], &window[1]);
        if let Some(&value) = probability.get(&pair)
            && value < cut_protect_probability
            && !protected.contains(&pair.0)
            && !protected.contains(&pair.1)
        {
            candidates.push((pair.0, pair.1, value));
        }
    }
    candidates.into_iter().min_by(|a, b| {
        (a.2, &a.0, &a.1)
            .partial_cmp(&(b.2, &b.0, &b.1))
            .expect("finite probabilities")
    })
}

/// Node -> component index over the live edge set (union-find; for the same-
/// component check only — index values are irrelevant). `nodes` seeds isolated
/// endpoints as their own component.
fn components(edges: &[PathEdge], nodes: &BTreeSet<String>) -> BTreeMap<String, usize> {
    let mut universe: BTreeSet<&str> = nodes.iter().map(String::as_str).collect();
    for (l, r, _) in edges {
        universe.insert(l);
        universe.insert(r);
    }
    let ids: Vec<&str> = universe.iter().copied().collect();
    let index: BTreeMap<&str, usize> = ids.iter().enumerate().map(|(i, &s)| (s, i)).collect();
    let mut parent: Vec<usize> = (0..ids.len()).collect();
    fn find(parent: &mut [usize], mut x: usize) -> usize {
        while parent[x] != x {
            parent[x] = parent[parent[x]];
            x = parent[x];
        }
        x
    }
    for (l, r, _) in edges {
        let (a, b) = (
            find(&mut parent, index[l.as_str()]),
            find(&mut parent, index[r.as_str()]),
        );
        if a != b {
            parent[a] = b;
        }
    }
    ids.iter()
        .enumerate()
        .map(|(i, &s)| (s.to_string(), find(&mut parent, i)))
        .collect()
}

/// Cut until no active `never` pair is co-clustered (S4.4.2 step 6).
///
/// `protected` (PR 1.3c) is the set of record keys a steward has pinned as
/// never-cut: an edge touching one is off-limits to the cut, so a `never` pair
/// reachable only through protected records escalates rather than severing the
/// protected record. Empty on the base path ⇒ byte-identical to pre-1.3c.
pub fn never_cut_fixpoint(
    edges: &[PathEdge],
    assertions: &[Assertion],
    nodes: &[String],
    cut_protect_probability: f64,
    protected: &BTreeSet<String>,
    max_iterations: usize,
) -> Result<CutResult, NeverCutError> {
    let mut live: Vec<PathEdge> = edges.to_vec();
    let never: Vec<&Assertion> = assertions
        .iter()
        .filter(|a| a.kind == AssertionKind::Never && a.active)
        .collect();
    if never.is_empty() {
        return Ok(CutResult::default());
    }

    let mut cuts: Vec<Cut> = Vec::new();
    let mut escalated: Vec<(String, String, String)> = Vec::new();
    let mut escalated_pairs: BTreeSet<(String, String)> = BTreeSet::new();
    let mut every_node: BTreeSet<String> = nodes.iter().cloned().collect();
    for (l, r, _) in &live {
        every_node.insert(l.clone());
        every_node.insert(r.clone());
    }

    let mut iterations = 0;
    loop {
        let component = components(&live, &every_node);
        let violations: Vec<&Assertion> = never
            .iter()
            .copied()
            .filter(|a| {
                let pair = canon(&a.rec_a_key, &a.rec_b_key);
                !escalated_pairs.contains(&pair)
                    && matches!(
                        (component.get(&a.rec_a_key), component.get(&a.rec_b_key)),
                        (Some(x), Some(y)) if x == y
                    )
            })
            .collect();
        if violations.is_empty() {
            return Ok(CutResult {
                cuts,
                escalations: escalated,
                iterations,
            });
        }
        if iterations >= max_iterations {
            return Err(NeverCutError::NonConvergence {
                max_iterations,
                remaining: violations.len(),
            });
        }

        iterations += 1;
        let mut progressed = false;
        for assertion in violations {
            let Some(path) = shortest_path(&assertion.rec_a_key, &assertion.rec_b_key, &live)
            else {
                continue;
            };
            match choose_cut_edge(&path, &live, cut_protect_probability, protected) {
                None => {
                    let pair = canon(&assertion.rec_a_key, &assertion.rec_b_key);
                    escalated.push((
                        pair.0.clone(),
                        pair.1.clone(),
                        assertion.assertion_id.clone(),
                    ));
                    escalated_pairs.insert(pair);
                    progressed = true;
                }
                Some(edge) => {
                    cuts.push(Cut {
                        rec_a_key: edge.0.clone(),
                        rec_b_key: edge.1.clone(),
                        match_probability: edge.2,
                        assertion_id: assertion.assertion_id.clone(),
                    });
                    let cut_pair = (edge.0.clone(), edge.1.clone());
                    live.retain(|(l, r, _)| canon(l, r) != cut_pair);
                    progressed = true;
                }
            }
        }
        if !progressed {
            return Err(NeverCutError::NoProgress(never.len()));
        }
    }
}
