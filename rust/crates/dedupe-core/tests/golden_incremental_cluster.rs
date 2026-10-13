//! Incremental-clustering equivalence gate.
//!
//! Connected components is a pure function of (nodes, edges): folding a new
//! batch's edges into an existing cluster assignment must yield exactly the
//! same partition as fully re-clustering the combined edge set. The full
//! re-cluster (`cluster_at_threshold`) is itself golden-verified (see
//! golden_cluster.rs), so it is a sound oracle here.

mod common;

use common::{FAKE_1000_SCHEMA, fake_1000_csv_path, golden_dir, load_golden_text};
use dedupe_core::Settings;
use dedupe_core::clustering::{ClusterOptions, cluster_at_threshold, incremental_cluster};
use dedupe_core::incremental::{predict_between, predict_within};
use dedupe_core::predict;
use dedupe_core::session::{Session, SessionOptions, TableHandle};

const THRESHOLD: f64 = 0.95;

fn register_parquet(session: &Session, name: &str, templated: &str) -> TableHandle {
    let path = golden_dir().join(name);
    let path = path.to_string_lossy().replace('\'', "''");
    session
        .register_table(&format!("read_parquet('{path}')"), templated)
        .unwrap()
}

/// UNION ALL two prediction tables down to the three columns clustering needs.
fn union_edges(
    session: &Session,
    a: &TableHandle,
    b: &TableHandle,
    templated: &str,
) -> TableHandle {
    let cols = "unique_id_l, unique_id_r, match_probability";
    let subquery = format!(
        "(select {cols} from {} union all select {cols} from {})",
        a.physical, b.physical
    );
    session.register_table(&subquery, templated).unwrap()
}

fn opts() -> ClusterOptions {
    ClusterOptions {
        threshold_match_probability: THRESHOLD,
    }
}

#[test]
fn incremental_cluster_equals_full_recluster() {
    let session = Session::open_in_memory(SessionOptions::deterministic()).unwrap();
    let settings = Settings::from_json_str(&load_golden_text("model_final.json")).unwrap();

    // Existing (900), new (100), and the full set (1000) of nodes.
    let existing_input = register_parquet(&session, "incremental_existing.parquet", "existing");
    let new_input = register_parquet(&session, "incremental_new.parquet", "new");
    let full_input = session
        .register_csv(&fake_1000_csv_path(), "fake_1000", FAKE_1000_SCHEMA)
        .unwrap();

    let existing_concat = predict::materialize_concat(&session, &existing_input).unwrap();
    let new_concat = predict::materialize_concat(&session, &new_input).unwrap();
    let all_concat = predict::materialize_concat(&session, &full_input).unwrap();
    let tf = predict::materialize_tf_tables(&session, &settings, &existing_concat).unwrap();

    // Existing state: within-existing edges, clustered.
    let existing_predictions =
        predict_within(&session, &settings, &existing_concat, &tf, None).unwrap();
    let existing_clusters = cluster_at_threshold(
        &session,
        &settings,
        &existing_concat,
        &existing_predictions,
        opts(),
    )
    .unwrap();

    // New batch edges: new-vs-existing and within-new.
    let between = predict_between(
        &session,
        &settings,
        &existing_concat,
        &new_concat,
        &tf,
        None,
    )
    .unwrap();
    let within_new = predict_within(&session, &settings, &new_concat, &tf, None).unwrap();
    let new_edges = union_edges(&session, &between, &within_new, "new_edges");

    // Incremental: fold new edges into the existing assignment.
    let incremental = incremental_cluster(
        &session,
        &settings,
        &all_concat,
        &existing_clusters,
        &new_edges,
        opts(),
    )
    .unwrap();

    // Oracle: fully re-cluster the combined edge set over all nodes.
    let all_edges = union_edges(&session, &existing_predictions, &new_edges, "all_edges");
    let oracle =
        cluster_at_threshold(&session, &settings, &all_concat, &all_edges, opts()).unwrap();

    // Both must cover all 1000 nodes and agree on every assignment.
    let inc_rows = session.count(&incremental.physical).unwrap();
    let oracle_rows = session.count(&oracle.physical).unwrap();
    assert_eq!(inc_rows, 1000, "incremental must assign every node");
    assert_eq!(oracle_rows, 1000, "oracle must assign every node");

    let mismatches = session
        .query_scalar_f64(&format!(
            "select count(*)::double from {inc} i full outer join {orc} o on i.node_id = o.node_id \
             where i.cluster_id is distinct from o.cluster_id or i.node_id is null or o.node_id is null",
            inc = incremental.physical,
            orc = oracle.physical,
        ))
        .unwrap() as i64;
    assert_eq!(
        mismatches, 0,
        "incremental clustering must match a full re-cluster of the same edges"
    );

    // Sanity: the new batch actually folded in — at least one new record
    // (id >= 900) was placed into a pre-existing cluster (cluster_id < 900),
    // so this is not a trivial pass. (New nodes joining an existing cluster
    // don't change existing nodes' cluster ids, since the cluster id is the
    // minimum node id and new ids are larger.)
    let joined_existing = session
        .query_scalar_f64(&format!(
            "select count(*)::double from {inc} where node_id >= 900 and cluster_id < 900",
            inc = incremental.physical,
        ))
        .unwrap() as i64;
    assert!(
        joined_existing > 0,
        "expected at least one new record to join an existing cluster"
    );

    // And the within-new edges should have merged some new records together.
    let new_multi = session
        .query_scalar_f64(&format!(
            "select count(*)::double from (select cluster_id from {inc} where node_id >= 900 \
             group by cluster_id having count(*) > 1)",
            inc = incremental.physical,
        ))
        .unwrap() as i64;
    assert!(
        new_multi > 0,
        "expected the new batch to form multi-record clusters"
    );
}
