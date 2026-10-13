//! S4.5.2 min-label propagation — the clustering partition (port of
//! `er.entities.cluster.label_propagate`).
//!
//! `label(v) = min(record_key)` over `v`'s connected component: every record is
//! labelled with the lexicographically smallest `record_key` reachable from it
//! through the (already probability-filtered, assertion-adjusted) edge set. The
//! result is independent of input order and pair orientation — adjacency is
//! symmetric and the label is a `min` over a set — which is what D1/D2 (S4.5.4)
//! require of the partition this feeds. Isolated nodes label themselves.
//!
//! The Python impl runs this as a bounded SQL pointer-jump fixpoint; the partition
//! (the only thing that leaves) is exactly the connected components with min-key
//! labels, so a union-find computes byte-identical `labels`.

use std::collections::BTreeMap;

/// Disjoint-set over record-key indices, union by first-seen, with path halving.
struct UnionFind {
    parent: Vec<usize>,
}

impl UnionFind {
    fn new(n: usize) -> Self {
        Self {
            parent: (0..n).collect(),
        }
    }

    fn find(&mut self, mut x: usize) -> usize {
        while self.parent[x] != x {
            self.parent[x] = self.parent[self.parent[x]];
            x = self.parent[x];
        }
        x
    }

    fn union(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            self.parent[ra] = rb;
        }
    }
}

/// Label every node with the min `record_key` of its connected component.
/// `nodes` seeds the universe (so an isolated record still appears, labelling
/// itself); `edges` are unordered `(a, b)` record-key pairs.
#[must_use]
pub fn label_propagate(nodes: &[String], edges: &[(String, String)]) -> BTreeMap<String, String> {
    // The node universe: `nodes` plus every edge endpoint, deduped and ordered.
    let mut ids: Vec<&str> = Vec::new();
    for node in nodes {
        ids.push(node.as_str());
    }
    for (a, b) in edges {
        ids.push(a.as_str());
        ids.push(b.as_str());
    }
    ids.sort_unstable();
    ids.dedup();
    let index: BTreeMap<&str, usize> = ids.iter().enumerate().map(|(i, &s)| (s, i)).collect();

    let mut uf = UnionFind::new(ids.len());
    for (a, b) in edges {
        uf.union(index[a.as_str()], index[b.as_str()]);
    }

    // Per component root, the min record_key (ids are sorted, so the first id seen
    // for a root is already its min).
    let mut root_min: BTreeMap<usize, &str> = BTreeMap::new();
    for (i, &s) in ids.iter().enumerate() {
        let root = uf.find(i);
        root_min.entry(root).or_insert(s);
    }

    ids.iter()
        .enumerate()
        .map(|(i, &s)| {
            let root = uf.find(i);
            (s.to_string(), root_min[&root].to_string())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| (*x).to_string()).collect()
    }

    #[test]
    fn chain_and_singletons() {
        let labels = label_propagate(
            &s(&["a", "b", "c", "z"]),
            &[("a".into(), "b".into()), ("b".into(), "c".into())],
        );
        assert_eq!(labels["a"], "a");
        assert_eq!(labels["c"], "a");
        assert_eq!(labels["z"], "z"); // isolated labels itself
    }
}
