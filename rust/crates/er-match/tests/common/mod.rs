// Shared test helpers: each integration-test binary compiles this module and
// uses a different subset, so unused-in-one-binary items are expected.
#![allow(dead_code)]

//! Shared setup for the matching/golden/scenario gates: land deliveries,
//! standardize, and run the resolution (match -> cluster -> reconcile).
//! Standardization is gated byte-exact vs Python in `er-standardize`
//! (std_hashes.rs) and matching at 1e-10 in `match_scores.rs`, so the
//! downstream gates take those as given.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use dedupe_core::predict::run_predict_with_tf;
use dedupe_core::session::Session;
use er_config::{Config, load_config_str};
use er_ids::{CountingIdFactory, IdFactory};
use er_ingest::landing::{create_raw_records_sql, land_base_delivery};
use er_match::{build_frozen_tf_tables, load_model};
use er_reconcile::assertions::{Assertion, AssertionKind};
use er_reconcile::cluster::label_propagate;
use er_reconcile::never_cut::{Cut, PathEdge, never_cut_fixpoint};
use er_reconcile::reconcile::{PlannedEvent, ReconcilePlan, reconcile_plan};
use er_standardize::build_int_std_records;

pub const MODEL_VERSION: &str = "v0001";
pub const TF_SNAPSHOT_ID: &str = "01JWMDTFSNAP00000000000001";
pub const AUTO_MERGE: f64 = 0.95; // configs/test.yaml thresholds.auto_merge
const MODEL_JSON: &str = include_str!("../fixtures/model_test_v1.json");
const TF_CSV: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/model_test_v1.tf.csv"
);
/// The canonical scenario fixtures (fixtures/static), read at runtime so every
/// scenario need not be vendored into this crate.
const REPO_FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../fixtures/static");

fn test_config() -> Config {
    load_config_str(include_str!("../fixtures/test.yaml")).unwrap()
}

/// Create `raw_records` and seed the nickname lexicon `name_variants` reads.
pub fn init_raw(session: &Session) {
    session
        .execute_batch(&create_raw_records_sql("raw_records"))
        .unwrap();
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
}

/// Append one scenario phase's deliveries to `raw_records`, reading whichever
/// `fixtures/static/<scenario>/<phase>/<source>.csv` exist (a phase may deliver
/// a subset of sources). Returns the sources landed.
pub fn land_phase(
    session: &Session,
    config: &Config,
    scenario: &str,
    phase: &str,
    ingest_batch_id: &str,
    ingested_at: &str,
) -> Vec<String> {
    let mut landed = Vec::new();
    for (source, spec) in &config.sources {
        let path = format!("{REPO_FIXTURES}/{scenario}/{phase}/{source}.csv");
        let Ok(csv_text) = std::fs::read_to_string(&path) else {
            continue;
        };
        land_base_delivery(
            session,
            "raw_records",
            source,
            spec,
            &csv_text,
            ingest_batch_id,
            ingested_at,
        )
        .unwrap();
        landed.push(source.clone());
    }
    landed
}

/// (Re)build `int_std_records` from the current `raw_records`.
pub fn rebuild_std(session: &Session, config: &Config) {
    session
        .execute_batch("drop table if exists int_std_records")
        .unwrap();
    build_int_std_records(
        session,
        config,
        "raw_records",
        "nickname_variants",
        "int_std_records",
    )
    .unwrap();
}

/// Match the current `int_std_records` with frozen TF, cluster the auto-merge
/// pairs into connected components, and reconcile against `p_old` (the prior
/// active membership; empty for a base phase). `id_factory` mints new entity
/// ids and MUST be threaded across a scenario's phases (one instance for base
/// then batch) so a later phase's new entities never collide with earlier ids.
pub fn resolve_with_prior(
    session: &Session,
    config: &Config,
    p_old: &BTreeMap<String, Vec<String>>,
    id_factory: &dyn IdFactory,
    never_assertions: &[Assertion],
) -> ReconcilePlan {
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

    // Auto-merge edges (canonical, deduped) with their probabilities — never-cut
    // chooses the lowest-probability edge on a path, so the weight is load-bearing.
    let mut edges: Vec<PathEdge> = session
        .query_strings(&format!(
            "select distinct on (rec_a_key, rec_b_key) rec_a_key, rec_b_key, match_probability
               from (
                     select least(record_key_l, record_key_r) as rec_a_key,
                            greatest(record_key_l, record_key_r) as rec_b_key,
                            match_probability
                       from {predictions}
                      where record_key_l <> record_key_r and match_probability >= {AUTO_MERGE}
                   )
              order by rec_a_key, rec_b_key, match_probability desc",
            predictions = predictions.physical,
        ))
        .unwrap()
        .into_iter()
        .map(|r| (r[0].clone(), r[1].clone(), r[2].parse::<f64>().unwrap()))
        .collect();

    let nodes: Vec<String> = session
        .query_strings("select record_key from int_std_records order by record_key")
        .unwrap()
        .into_iter()
        .map(|r| r[0].clone())
        .collect();

    // S4.4 edge adjustment: suppress the DIRECT edge of every `never` pair before
    // clustering. A bridge (the pair is a component's only connection) splits
    // outright here — no partition-level cut, so no `edge_cut` event.
    let never_pairs: BTreeSet<(String, String)> = never_assertions
        .iter()
        .map(|a| {
            let (x, y) = (a.rec_a_key.clone(), a.rec_b_key.clone());
            if x <= y { (x, y) } else { (y, x) }
        })
        .collect();
    edges.retain(|(a, b, _)| !never_pairs.contains(&(a.clone(), b.clone())));

    // S4.4.2 never-cut: a `never` pair still co-clustered through OTHER records
    // forces a partition-level cut of the lowest-probability edge on the path.
    // Each such cut (unlike the direct-edge adjustment above) is an `edge_cut`.
    let mut cuts: Vec<Cut> = Vec::new();
    if !never_assertions.is_empty() {
        cuts = never_cut_fixpoint(
            &edges,
            never_assertions,
            &nodes,
            config.clustering.cut_protect_probability,
            config.clustering.max_iterations.try_into().unwrap(),
        )
        .unwrap()
        .cuts;
        let cutset: BTreeSet<(String, String)> = cuts
            .iter()
            .map(|c| (c.rec_a_key.clone(), c.rec_b_key.clone()))
            .collect();
        edges.retain(|(a, b, _)| !cutset.contains(&(a.clone(), b.clone())));
    }

    let plain_edges: Vec<(String, String)> = edges
        .iter()
        .map(|(a, b, _)| (a.clone(), b.clone()))
        .collect();
    let labels = label_propagate(&nodes, &plain_edges);
    let mut by_label: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (record_key, label) in &labels {
        by_label
            .entry(label.clone())
            .or_default()
            .push(record_key.clone());
    }
    let p_new: Vec<Vec<String>> = by_label.into_values().collect();
    let mut plan = reconcile_plan(p_old, &p_new, id_factory);

    // S4.4.2 edge_cut events: one per cut, attributed to the prior entity it
    // severed. Both endpoints were co-clustered, so they share a prior entity,
    // which survives as the claimant fragment and carries the event.
    if !cuts.is_empty() {
        let mut prior_entity: BTreeMap<&str, &str> = BTreeMap::new();
        for (entity, members) in p_old {
            for m in members {
                prior_entity.insert(m.as_str(), entity.as_str());
            }
        }
        for c in &cuts {
            if let Some(entity) = prior_entity.get(c.rec_a_key.as_str()) {
                plan.events.push(PlannedEvent {
                    entity_id: (*entity).to_string(),
                    event_type: "edge_cut".to_string(),
                    details: serde_json::Value::Null,
                    details_hash: String::new(),
                });
            }
        }
    }
    plan
}

/// Fold a reconcile plan onto the prior membership to get the full new
/// membership (entity_id -> member record_keys). `reconcile_plan` returns only
/// the DELTA (the set-equal fast path emits nothing for unchanged groups), so
/// the full partition is `p_old` with each assignment's record moved to its new
/// entity; merged/retired entities keep no members (all were reassigned). This
/// is both the batch partition under test and the `p_old` for any next phase.
///
/// Note: this does not yet drop tombstoned records (member_removed) — fine for
/// base/merge/split scenarios, revisit for deletion_scenario.
pub fn fold_plan(
    p_old: &BTreeMap<String, Vec<String>>,
    plan: &ReconcilePlan,
) -> BTreeMap<String, Vec<String>> {
    let mut record_entity: BTreeMap<String, String> = BTreeMap::new();
    for (entity, members) in p_old {
        for m in members {
            record_entity.insert(m.clone(), entity.clone());
        }
    }
    for a in &plan.assignments {
        record_entity.insert(a.record_key.clone(), a.entity_id.clone());
    }
    let mut by_entity: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (record, entity) in record_entity {
        by_entity.entry(entity).or_default().push(record);
    }
    by_entity
}

/// The assertions a scenario declares for a phase, read from
/// `fixtures/static/<scenario>/assertions.csv` (columns: phase, rec_a_key,
/// rec_b_key, kind, created_by, note). All are `active` (the retraction case is
/// not modeled yet); the synthesized `assertion_id` is only referenced by the
/// cut/contradiction records, which these gates do not compare. Empty when the
/// scenario has no assertions file.
pub fn load_assertions(scenario: &str, phase: &str) -> Vec<Assertion> {
    let path = format!("{REPO_FIXTURES}/{scenario}/assertions.csv");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for line in text.lines().skip(1).filter(|l| !l.is_empty()) {
        let f: Vec<&str> = line.split(',').collect();
        if f[0] != phase {
            continue;
        }
        let kind = match f[3] {
            "always" => AssertionKind::Always,
            "never" => AssertionKind::Never,
            other => panic!("unknown assertion kind {other:?} in {path}"),
        };
        out.push(Assertion {
            assertion_id: format!("{}|{}|{}", f[1], f[2], f[3]),
            rec_a_key: f[1].to_string(),
            rec_b_key: f[2].to_string(),
            kind,
            active: true,
        });
    }
    out
}

/// Initialize a session for a multi-phase scenario: create `raw_records` + seed
/// the lexicon, returning the test config. The caller lands phases itself (via
/// [`land_phase`] + [`rebuild_std`] + [`resolve_with_prior`]).
pub fn scenario_init(session: &Session) -> Config {
    let config = test_config();
    init_raw(session);
    config
}

/// Land base_10's base delivery and build `int_std_records` (the std/match/golden
/// gates' entry point). Returns the config.
pub fn land_and_standardize_base_10(session: &Session) -> Config {
    let config = test_config();
    init_raw(session);
    land_phase(
        session,
        &config,
        "base_10",
        "base",
        "01BATCH0000000000000000BASE",
        "2026-01-01 00:00:00",
    );
    rebuild_std(session, &config);
    config
}

/// Full base_10 resolution (base phase, empty prior). Leaves `int_std_records`
/// in the session; returns the config and the plan.
pub fn resolve_base_10(session: &Session) -> (Config, ReconcilePlan) {
    let config = land_and_standardize_base_10(session);
    let plan = resolve_with_prior(
        session,
        &config,
        &BTreeMap::new(),
        &CountingIdFactory::new(0),
        &[],
    );
    (config, plan)
}

/// Materialize `entity_membership(record_key, entity_id)` and
/// `entities(entity_id, status)` from a reconcile plan, so the golden stage's SQL
/// can join them exactly as it joins the lake relations.
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
