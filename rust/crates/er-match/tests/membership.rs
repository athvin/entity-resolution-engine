//! The end-to-end entity-membership gate: standardize -> match -> cluster ->
//! reconcile, and assert the resulting partition equals base_10's committed
//! `membership.csv` (ID-insensitively — Rust mints its own entity ids).
//!
//! This is T-INV-1 in miniature ("membership equals connected components"):
//! records are auto-merged (`match_probability >= auto_merge`) into connected
//! components by `label_propagate`, and `reconcile_plan` (empty prior) mints one
//! entity per component. The discriminating trap is crm:C006 — auto-merged to
//! billing:B005 (E5) but NOT to webforms:W006, because crm:C006–webforms:W006 is
//! the single gray-band pair (0.857 < auto_merge), so the shared household stays
//! two entities.

mod common;

use std::collections::{BTreeMap, BTreeSet};

use dedupe_core::predict::run_predict_with_tf;
use dedupe_core::session::{Session, SessionOptions};
use er_ids::CountingIdFactory;
use er_match::{build_frozen_tf_tables, load_model};
use er_reconcile::cluster::label_propagate;
use er_reconcile::reconcile::reconcile_plan;

const MODEL_VERSION: &str = "v0001";
const TF_SNAPSHOT_ID: &str = "01JWMDTFSNAP00000000000001";
const AUTO_MERGE: f64 = 0.95; // configs/test.yaml thresholds.auto_merge
const TF_CSV: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/model_test_v1.tf.csv"
);

/// The partition as a set of clusters, each a sorted set of record keys. Entity
/// labels are dropped, so Rust's minted ids and the golden's `E*` labels compare
/// equal whenever they induce the same grouping.
fn partition<I: IntoIterator<Item = (String, String)>>(pairs: I) -> BTreeSet<BTreeSet<String>> {
    let mut by_entity: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (entity, record_key) in pairs {
        by_entity.entry(entity).or_default().insert(record_key);
    }
    by_entity.into_values().collect()
}

#[test]
fn base_10_membership_matches_committed_partition() {
    let session = Session::open_in_memory(SessionOptions::deterministic()).unwrap();
    common::land_and_standardize_base_10(&session);

    let settings = load_model(include_str!("fixtures/model_test_v1.json")).unwrap();
    let input = session
        .register_table("int_std_records", "int_std_records")
        .unwrap();
    let tf_tables = build_frozen_tf_tables(
        &session,
        &settings,
        std::path::Path::new(TF_CSV),
        MODEL_VERSION,
        TF_SNAPSHOT_ID,
    )
    .unwrap();
    let predictions = run_predict_with_tf(&session, &settings, &input, &tf_tables, None).unwrap();

    // Auto-merge edges: canonical pairs at or above auto_merge (the gray band is
    // explicitly excluded — it goes to review, not to clustering).
    let edges: Vec<(String, String)> = session
        .query_strings(&format!(
            "select distinct rec_a_key, rec_b_key from (
                 select least(record_key_l, record_key_r) as rec_a_key,
                        greatest(record_key_l, record_key_r) as rec_b_key
                   from {predictions}
                  where record_key_l <> record_key_r and match_probability >= {AUTO_MERGE}
               ) order by rec_a_key, rec_b_key",
            predictions = predictions.physical,
        ))
        .unwrap()
        .into_iter()
        .map(|r| (r[0].clone(), r[1].clone()))
        .collect();

    // The full node universe, so singletons (e.g. crm:C008) form their own cluster.
    let nodes: Vec<String> = session
        .query_strings("select record_key from int_std_records order by record_key")
        .unwrap()
        .into_iter()
        .map(|r| r[0].clone())
        .collect();
    assert_eq!(
        nodes.len(),
        23,
        "base_10 base delivery is 23 current records"
    );

    // Connected components -> clusters -> mint one entity each (empty prior).
    let labels = label_propagate(&nodes, &edges);
    let mut by_label: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (record_key, label) in &labels {
        by_label
            .entry(label.clone())
            .or_default()
            .push(record_key.clone());
    }
    let p_new: Vec<Vec<String>> = by_label.into_values().collect();
    let plan = reconcile_plan(&BTreeMap::new(), &p_new, &CountingIdFactory::new(0));

    let got = partition(
        plan.assignments
            .iter()
            .map(|a| (a.entity_id.clone(), a.record_key.clone())),
    );

    // Expected partition from the committed golden: group record_key by entity_label.
    let want = partition(
        include_str!("fixtures/membership_base_10.csv")
            .lines()
            .skip(1)
            .filter(|l| !l.is_empty())
            .map(|line| {
                let f: Vec<&str> = line.split(',').collect();
                // persona_id, source_system, source_record_id, entity_label
                (f[3].to_string(), format!("{}:{}", f[1], f[2]))
            }),
    );

    assert_eq!(
        got.len(),
        10,
        "base_10 resolves to 10 entities; got {}",
        got.len()
    );
    assert_eq!(
        got, want,
        "membership partition differs from the committed golden"
    );
}
