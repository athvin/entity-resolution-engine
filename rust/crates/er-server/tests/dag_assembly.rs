//! The DAG registry's own gate (batch-orchestration-design §3.3): every kind
//! assembles, every lake edge names only declared relations, the schedulable
//! list is a subset of the kind vocabulary, and the two fingerprints behave.

use std::collections::BTreeSet;

use er_server::dags::{self, JobKind};

#[test]
fn every_kind_assembles_and_the_registry_is_total() {
    for kind in JobKind::ALL {
        let dag = dags::dag_for(kind);
        assert!(!dag.nodes.is_empty(), "{} has an empty DAG", kind.as_str());
        assert_eq!(dag.name, kind.dag_name(), "{}", kind.as_str());
        assert_eq!(dag.structural_fingerprint.len(), 64);
        assert_eq!(dag.policy_hash.len(), 64);
    }
    // One graph, two kinds: the mode is a run parameter, not a shape (§3.5).
    assert_eq!(
        dags::dag_for(JobKind::RunAllFull).structural_fingerprint,
        dags::dag_for(JobKind::RunAllIncremental).structural_fingerprint,
    );
    // Distinct kinds carry distinct structures.
    let distinct: BTreeSet<String> = JobKind::ALL
        .into_iter()
        .map(|k| dags::dag_for(k).structural_fingerprint)
        .collect();
    assert_eq!(distinct.len(), JobKind::ALL.len() - 1);
}

#[test]
fn every_lake_edge_names_a_declared_relation() {
    // The assembly-tier mirror of `declared_names.rs` (§3.2): an edge naming a
    // relation the S5 registry does not declare is refused here, before any
    // runtime exists to discover it.
    let declared: BTreeSet<String> = er_schema::model::all_specs()
        .into_iter()
        .map(|s| s.name)
        .collect();
    let edges = dags::edges::all();
    assert!(edges.len() >= 6, "the edge vocabulary shrank");
    for (edge, relations) in edges {
        assert!(!relations.is_empty(), "edge {edge} is empty");
        for rel in relations {
            assert!(
                declared.contains(*rel),
                "edge {edge} names relation {rel}, which is not in the S5 registry"
            );
        }
    }
}

#[test]
fn the_kind_vocabulary_matches_the_queue() {
    // Every schedulable kind parses into the registry, so a schedule can never
    // enqueue a kind planning would refuse as unparseable.
    for kind in er_server::SCHEDULABLE_KINDS {
        assert!(
            JobKind::parse(kind).is_some(),
            "SCHEDULABLE_KINDS carries '{kind}', which JobKind does not parse"
        );
    }
    // The one unscheduled kind the product enqueues (dispatch_coverage.rs's
    // UNSCHEDULED_KINDS) parses too.
    assert!(JobKind::parse("apply_corrections").is_some());
    // Round trip, and the unknown kind is refused.
    for kind in JobKind::ALL {
        assert_eq!(JobKind::parse(kind.as_str()), Some(kind));
    }
    assert_eq!(JobKind::parse("not_a_real_kind"), None);
}

#[test]
fn the_run_all_graph_matches_the_design() {
    let dag = dags::dag_for(JobKind::RunAllFull);
    let names: Vec<&str> = dag.nodes.iter().map(|n| n.task_id).collect();
    assert_eq!(
        names,
        vec!["init", "ingest", "standardize", "match", "reconcile", "assemble", "finish"]
    );
    let finish = &dag.nodes[6];
    assert!(finish.teardown);
    assert_eq!(finish.trigger_rule.as_str(), "all_terminal");
    // Every lake writer is ordered against every other (the §3.3 single-writer
    // check holds by construction on the chain).
    assert!(dag.nodes.iter().take(6).all(|n| n.writes_lake));
    // match and reconcile escalate on OOM by default (§5.5).
    assert!(dag.nodes[3].policy.escalate_on_oom);
    assert!(dag.nodes[4].policy.escalate_on_oom);
}

#[test]
fn control_tasks_carry_the_control_policy() {
    let dag = dags::dag_for(JobKind::Train);
    let (_, register) = dag
        .node_by_task_id("register_model")
        .expect("train registers its model");
    assert_eq!(register.kind.as_str(), "control");
    assert_eq!(register.policy.backoff.base_secs, 5);
    assert_eq!(register.policy.backoff.cap_secs, 60);
    assert_eq!(register.policy.timeout_secs, Some(60));
    assert!(!register.writes_lake);
}
