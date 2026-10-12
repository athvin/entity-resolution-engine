//! Batch-2 golden gates: graph metrics (nodes + clusters), multi-threshold
//! cluster summary, and single-best-links — all vs the committed goldens.

mod common;

use common::{
    FAKE_1000_SCHEMA, assert_table_matches_parquet, fake_1000_csv_path, golden_dir,
    load_golden_text,
};
use dedupe_core::Settings;
use dedupe_core::clustering::{
    ClusterOptions, cluster_at_threshold, cluster_summary_at_thresholds,
    cluster_using_single_best_links, clustered_with_input_data, edge_bridges, graph_metrics,
};
use dedupe_core::predict;
use dedupe_core::session::{Session, SessionOptions, TableHandle};

const THRESHOLD: f64 = 0.95;

fn reg(session: &Session, name: &str, templated: &str) -> TableHandle {
    let p = golden_dir().join(name);
    let p = p.to_string_lossy().replace('\'', "''");
    session
        .register_table(&format!("read_parquet('{p}')"), templated)
        .unwrap()
}

#[test]
fn graph_metrics_match_golden() {
    let session = Session::open_in_memory(SessionOptions::deterministic()).unwrap();
    let input = session
        .register_csv(&fake_1000_csv_path(), "fake_1000", FAKE_1000_SCHEMA)
        .unwrap();
    let settings = Settings::from_json_str(&load_golden_text("model_final.json")).unwrap();
    let predictions = predict::run_predict(&session, &settings, &input, None).unwrap();
    let concat = predict::materialize_concat(&session, &input).unwrap();
    let clusters = cluster_at_threshold(
        &session,
        &settings,
        &concat,
        &predictions,
        ClusterOptions {
            threshold_match_probability: THRESHOLD,
        },
    )
    .unwrap();
    let clustered = clustered_with_input_data(&session, &settings, &concat, &clusters).unwrap();

    let (nodes, cluster_metrics) =
        graph_metrics(&session, &settings, &predictions, &clustered, THRESHOLD).unwrap();

    assert_table_matches_parquet(
        &session,
        &nodes,
        "graph_metrics_nodes.parquet",
        &["composite_unique_id"],
        &["cluster_id", "node_degree"],
        &[("node_centrality", 1e-9)],
    );
    assert_table_matches_parquet(
        &session,
        &cluster_metrics,
        "graph_metrics_clusters.parquet",
        &["cluster_id"],
        &["n_nodes"],
        &[
            ("n_edges", 1e-9),
            ("density", 1e-9),
            ("cluster_centralisation", 1e-9),
        ],
    );
}

#[test]
fn edge_bridges_match_golden() {
    let session = Session::open_in_memory(SessionOptions::deterministic()).unwrap();
    let input = session
        .register_csv(&fake_1000_csv_path(), "fake_1000", FAKE_1000_SCHEMA)
        .unwrap();
    let settings = Settings::from_json_str(&load_golden_text("model_final.json")).unwrap();
    let predictions = predict::run_predict(&session, &settings, &input, None).unwrap();

    let edges = edge_bridges(&session, &settings, &predictions, THRESHOLD).unwrap();
    assert_table_matches_parquet(
        &session,
        &edges,
        "graph_metrics_edges.parquet",
        &["composite_unique_id_l", "composite_unique_id_r"],
        &["is_bridge"],
        &[],
    );
}

#[test]
fn multi_threshold_summary_matches_golden() {
    let session = Session::open_in_memory(SessionOptions::deterministic()).unwrap();
    let input = session
        .register_csv(&fake_1000_csv_path(), "fake_1000", FAKE_1000_SCHEMA)
        .unwrap();
    let settings = Settings::from_json_str(&load_golden_text("model_final.json")).unwrap();
    let predictions = predict::run_predict(&session, &settings, &input, None).unwrap();
    let concat = predict::materialize_concat(&session, &input).unwrap();

    let summary = cluster_summary_at_thresholds(
        &session,
        &settings,
        &concat,
        &predictions,
        &[0.5, 0.9, 0.95, 0.99],
    )
    .unwrap();

    assert_table_matches_parquet(
        &session,
        &summary,
        "multi_threshold_summary.parquet",
        &["threshold_match_probability"],
        &["num_clusters", "max_cluster_size"],
        &[("threshold_match_weight", 1e-6), ("avg_cluster_size", 1e-9)],
    );
}

#[test]
fn single_best_links_matches_golden() {
    let session = Session::open_in_memory(SessionOptions::deterministic()).unwrap();
    let a = reg(&session, "link_df_a.parquet", "df_a");
    let b = reg(&session, "link_df_b.parquet", "df_b");
    let settings = Settings::from_json_str(&load_golden_text("link_link_only_model.json")).unwrap();
    let concat =
        predict::materialize_concat_multi(&session, &[(&a, "df_a"), (&b, "df_b")], &settings)
            .unwrap();
    let tf = predict::materialize_tf_tables(&session, &settings, &concat).unwrap();
    let blocked = predict::materialize_blocked_pairs(&session, &settings, &concat).unwrap();
    let pipeline = predict::predict_pipeline(&settings, &concat, &tf, &blocked, None).unwrap();
    let predictions = session.materialize(&pipeline).unwrap();

    let clusters = cluster_using_single_best_links(
        &session,
        &settings,
        &concat,
        &predictions,
        &["df_a".to_string(), "df_b".to_string()],
        THRESHOLD,
        "lowest_id",
    )
    .unwrap();
    let joined = clustered_with_input_data(&session, &settings, &concat, &clusters).unwrap();

    assert_table_matches_parquet(
        &session,
        &joined,
        "single_best_links_clusters.parquet",
        &["source_dataset", "unique_id"],
        &["cluster_id"],
        &[],
    );
}
