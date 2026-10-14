//! The S4.4 assertion types shared by the never-cut fixpoint and the
//! contradiction check (port of `src/er/review/assertions.py`'s data shape +
//! `check_contradiction_1`).

use std::collections::{BTreeMap, BTreeSet};

/// `always` forces a merge; `never` forbids a co-cluster (S4.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssertionKind {
    Always,
    Never,
}

/// One steward assertion over a canonical record pair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assertion {
    pub assertion_id: String,
    pub rec_a_key: String,
    pub rec_b_key: String,
    pub kind: AssertionKind,
    pub active: bool,
}

/// One CONTRADICTION-1 finding: a `never` pair inside one `always`-closure
/// component, which is unsatisfiable as a partition (S4.4.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Contradiction {
    pub rec_a_key: String,
    pub rec_b_key: String,
    pub never_assertion_id: String,
    pub always_assertion_ids: Vec<String>,
    pub component: BTreeSet<String>,
}

/// Union-find root of `key` (string-keyed), with the lexically smaller root kept.
fn uf_find(parent: &mut BTreeMap<String, String>, key: &str) -> String {
    let mut root = key.to_string();
    loop {
        let next = parent[&root].clone();
        if next == root {
            break;
        }
        root = next;
    }
    let mut cursor = key.to_string();
    while parent[&cursor] != root {
        let next = parent[&cursor].clone();
        parent.insert(cursor, root.clone());
        cursor = next;
    }
    root
}

fn uf_union(parent: &mut BTreeMap<String, String>, a: &str, b: &str) {
    let (mut root_a, mut root_b) = (uf_find(parent, a), uf_find(parent, b));
    if root_a == root_b {
        return;
    }
    // Keep the lexically smaller root: the finding names a component, so the
    // representative must not depend on scan order (S4.4.1).
    if root_b < root_a {
        std::mem::swap(&mut root_a, &mut root_b);
    }
    parent.insert(root_b, root_a);
}

/// CONTRADICTION-1: every active `never` pair whose endpoints fall in one
/// `always`-closure component (S4.4.1). PURE. Returns one finding per offending
/// `never`, in canonical-pair order; empty when the set is satisfiable.
#[must_use]
pub fn check_contradiction_1(assertions: &[Assertion]) -> Vec<Contradiction> {
    let active: Vec<&Assertion> = assertions.iter().filter(|a| a.active).collect();
    let always: Vec<&Assertion> = active
        .iter()
        .copied()
        .filter(|a| a.kind == AssertionKind::Always)
        .collect();

    let mut parent: BTreeMap<String, String> = BTreeMap::new();
    for assertion in &always {
        for key in [&assertion.rec_a_key, &assertion.rec_b_key] {
            parent.entry(key.clone()).or_insert_with(|| key.clone());
        }
    }
    for assertion in &always {
        uf_union(&mut parent, &assertion.rec_a_key, &assertion.rec_b_key);
    }

    let keys: Vec<String> = parent.keys().cloned().collect();
    let mut components: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for key in &keys {
        let root = uf_find(&mut parent, key);
        components.entry(root).or_default().insert(key.clone());
    }
    let mut offenders: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for assertion in &always {
        let root = uf_find(&mut parent, &assertion.rec_a_key);
        offenders
            .entry(root)
            .or_default()
            .push(assertion.assertion_id.clone());
    }

    let mut found: Vec<Contradiction> = Vec::new();
    for assertion in &active {
        if assertion.kind != AssertionKind::Never {
            continue;
        }
        if !parent.contains_key(&assertion.rec_a_key) || !parent.contains_key(&assertion.rec_b_key)
        {
            continue;
        }
        let root = uf_find(&mut parent, &assertion.rec_a_key);
        if root != uf_find(&mut parent, &assertion.rec_b_key) {
            continue;
        }
        let mut always_assertion_ids = offenders[&root].clone();
        always_assertion_ids.sort();
        found.push(Contradiction {
            rec_a_key: assertion.rec_a_key.clone(),
            rec_b_key: assertion.rec_b_key.clone(),
            never_assertion_id: assertion.assertion_id.clone(),
            always_assertion_ids,
            component: components[&root].clone(),
        });
    }
    found.sort_by(|a, b| {
        (a.rec_a_key.as_str(), a.rec_b_key.as_str())
            .cmp(&(b.rec_a_key.as_str(), b.rec_b_key.as_str()))
    });
    found
}
