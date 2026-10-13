// Shared test helpers: each integration-test binary compiles this module and
// uses a different subset, so unused-in-one-binary items are expected.
#![allow(dead_code)]

//! Shared setup for the matching/golden gates: land base_10's base delivery,
//! standardize it, and (for the end-to-end gates) run the whole resolution
//! through reconcile. Standardization itself is gated byte-exact vs Python in
//! `er-standardize` (std_hashes.rs over this same delivery), and matching at
//! 1e-10 in `match_scores.rs`, so the downstream gates take those as given.

use std::collections::BTreeMap;
use std::path::Path;

use dedupe_core::predict::run_predict_with_tf;
use dedupe_core::session::Session;
use er_config::load_config_str;
use er_ids::CountingIdFactory;
use er_ingest::landing::{create_raw_records_sql, land_base_delivery};
use er_match::{build_frozen_tf_tables, load_model};
use er_reconcile::cluster::label_propagate;
use er_reconcile::reconcile::{ReconcilePlan, reconcile_plan};
use er_standardize::build_int_std_records;

pub const MODEL_VERSION: &str = "v0001";
pub const TF_SNAPSHOT_ID: &str = "01JWMDTFSNAP00000000000001";
pub const AUTO_MERGE: f64 = 0.95; // configs/test.yaml thresholds.auto_merge
const MODEL_JSON: &str = include_str!("../fixtures/model_test_v1.json");
const TF_CSV: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/model_test_v1.tf.csv"
);

/// Build `int_std_records` for base_10's base delivery into `session`, returning
/// the config. Leaves a table named `int_std_records` in the session.
pub fn land_and_standardize_base_10(session: &Session) -> er_config::Config {
    let config = load_config_str(include_str!("../fixtures/test.yaml")).unwrap();
    session
        .execute_batch(&create_raw_records_sql("raw_records"))
        .unwrap();

    // Seed the nickname lexicon the `name_variants` relation reads.
    let values: Vec<String> = include_str!("../fixtures/nickname_variants.csv")
        .lines()
        .skip(1)
        .filter(|l| !l.is_empty())
        .map(|line| {
            let (a, b) = line.split_once(',').unwrap();
            format!(
                "('{}', '{}', true)",
                a.replace('\'', "''"),
                b.replace('\'', "''")
            )
        })
        .collect();
    session
        .execute_batch(&format!(
            "create table nickname_variants as select * from (values {}) \
             as t(variant_a, variant_b, active)",
            values.join(", ")
        ))
        .unwrap();

    for (source, csv_text) in [
        ("crm", include_str!("../fixtures/base_10/base/crm.csv")),
        (
            "billing",
            include_str!("../fixtures/base_10/base/billing.csv"),
        ),
        (
            "webforms",
            include_str!("../fixtures/base_10/base/webforms.csv"),
        ),
    ] {
        let spec = &config.sources[source];
        land_base_delivery(
            session,
            "raw_records",
            source,
            spec,
            csv_text,
            "01BATCH0000000000000000BASE",
            "2026-01-01 00:00:00",
        )
        .unwrap();
    }

    build_int_std_records(
        session,
        &config,
        "raw_records",
        "nickname_variants",
        "int_std_records",
    )
    .unwrap();

    config
}

/// Full base_10 resolution: standardize -> frozen-TF match -> cluster
/// (connected components of pairs >= auto_merge) -> reconcile (empty prior). The
/// session is left holding `int_std_records`; returns the config and the plan,
/// whose `assignments`/`transitions` seed `entity_membership`/`entities`.
pub fn resolve_base_10(session: &Session) -> (er_config::Config, ReconcilePlan) {
    let config = land_and_standardize_base_10(session);

    let settings = load_model(MODEL_JSON).unwrap();
    let input = session
        .register_table("int_std_records", "int_std_records")
        .unwrap();
    let tf_tables = build_frozen_tf_tables(
        session,
        &settings,
        Path::new(TF_CSV),
        MODEL_VERSION,
        TF_SNAPSHOT_ID,
    )
    .unwrap();
    let predictions = run_predict_with_tf(session, &settings, &input, &tf_tables, None).unwrap();

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

    let nodes: Vec<String> = session
        .query_strings("select record_key from int_std_records order by record_key")
        .unwrap()
        .into_iter()
        .map(|r| r[0].clone())
        .collect();

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
    (config, plan)
}

/// Materialize `entity_membership(record_key, entity_id)` and
/// `entities(entity_id, status)` from a reconcile plan, so the golden stage's
/// SQL can join them exactly as it joins the lake relations.
pub fn materialize_entities(session: &Session, plan: &ReconcilePlan) {
    let membership: Vec<String> = plan
        .assignments
        .iter()
        .map(|a| {
            format!(
                "('{}', '{}')",
                a.record_key.replace('\'', "''"),
                a.entity_id.replace('\'', "''")
            )
        })
        .collect();
    session
        .execute_batch(&format!(
            "create table entity_membership as select * from (values {}) \
             as t(record_key, entity_id)",
            membership.join(", ")
        ))
        .unwrap();

    let entities: Vec<String> = plan
        .transitions
        .iter()
        .map(|t| {
            format!(
                "('{}', '{}')",
                t.entity_id.replace('\'', "''"),
                t.status.replace('\'', "''")
            )
        })
        .collect();
    session
        .execute_batch(&format!(
            "create table entities as select * from (values {}) as t(entity_id, status)",
            entities.join(", ")
        ))
        .unwrap();
}
