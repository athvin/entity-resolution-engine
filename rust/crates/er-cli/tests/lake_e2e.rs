//! P2 end-to-end gates (behind `DEDUPE_E2E_DUCKLAKE=1`, like dedupe-cli's
//! cli_e2e, since they install the `ducklake` extension): run the pipeline
//! stages over a REAL DuckLake catalog and assert the lake relations match
//! base_10's committed goldens — the same oracles the in-memory stage gates use,
//! now proving the lake orchestration end to end.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use dedupe_core::Session;
use er_config::{Config, load_config_str};
use er_hash::std_hash::{STD_HASH_COLUMNS, StdValue, table_content_hash};

/// base_10's committed match model (frozen settings + TF snapshot).
fn match_model(session: &Session) {
    let model_json = std::fs::read_to_string(repo_fixture("model_test_v1.json")).unwrap();
    let tf_csv = repo_fixture("model_test_v1.tf.csv");
    er_cli::run_match(
        session,
        &er_cli::MatchModel {
            settings_json: &model_json,
            tf_csv_path: std::path::Path::new(&tf_csv),
            model_version: "v0001",
            tf_snapshot_id: "01JWMDTFSNAP00000000000001",
        },
        0.60,
        "01RUNMATCH0000000000000000",
        "2026-01-02 00:00:00",
    )
    .unwrap();
}

/// Partition from `entity_id -> members` rows: set of clusters (id-insensitive).
fn partition(pairs: &[(String, String)]) -> BTreeSet<BTreeSet<String>> {
    let mut by: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (entity, member) in pairs {
        by.entry(entity.clone()).or_default().insert(member.clone());
    }
    by.into_values().collect()
}

fn enabled() -> bool {
    std::env::var("DEDUPE_E2E_DUCKLAKE").as_deref() == Ok("1")
}

fn repo_fixture(rel: &str) -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../fixtures/static")
        .join(rel)
        .to_string_lossy()
        .into_owned()
}

/// Open a fresh lake under a per-test temp dir (tests run in parallel) and seed
/// the nickname lexicon. Returns the session, config, and temp dir.
fn open_seeded_lake(tag: &str) -> (Session, Config, PathBuf) {
    let tmp = std::env::temp_dir().join(format!("er_cli_{tag}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(tmp.join("data")).unwrap();
    let catalog = tmp.join("catalog.ducklake").to_string_lossy().into_owned();
    let data_path = tmp.join("data").to_string_lossy().into_owned();

    let session = er_cli::open_lake(&catalog, &data_path).unwrap();
    let config = load_config_str(include_str!("fixtures/test.yaml")).unwrap();

    let lexicon: Vec<(String, String)> = include_str!("fixtures/nickname_variants.csv")
        .lines()
        .skip(1)
        .filter(|l| !l.is_empty())
        .map(|l| {
            let (a, b) = l.split_once(',').unwrap();
            (a.to_string(), b.to_string())
        })
        .collect();
    er_cli::seed_lexicon(&session, &lexicon).unwrap();
    (session, config, tmp)
}

/// Open a seeded lake and run ingest + standardize over base_10's base delivery.
fn standardized_lake(tag: &str) -> (Session, Config, PathBuf) {
    let (session, config, tmp) = open_seeded_lake(tag);
    for source in ["crm", "billing", "webforms"] {
        let csv =
            std::fs::read_to_string(repo_fixture(&format!("base_10/base/{source}.csv"))).unwrap();
        er_cli::run_ingest(
            &session,
            &config,
            source,
            &csv,
            "01BATCH0000000000000000BASE",
            "2026-01-01 00:00:00",
            false,
        )
        .unwrap();
    }
    er_cli::run_standardize(&session, &config).unwrap();
    (session, config, tmp)
}

/// Resolve one phase of a scenario through the lake: ingest every delivered
/// source, standardize, match, reconcile. Returns the membership partition.
fn resolve_phase_through_lake(
    session: &Session,
    config: &Config,
    scenario: &str,
    phase: &str,
    batch_id: &str,
    ts: &str,
) -> BTreeSet<BTreeSet<String>> {
    for source in ["crm", "billing", "webforms"] {
        let path = repo_fixture(&format!("{scenario}/{phase}/{source}.csv"));
        if let Ok(csv) = std::fs::read_to_string(&path) {
            er_cli::run_ingest(session, config, source, &csv, batch_id, ts, false).unwrap();
        }
    }
    er_cli::run_standardize(session, config).unwrap();
    match_model(session);
    er_cli::run_reconcile(session, config, batch_id, ts).unwrap();
    let rows: Vec<(String, String)> = session
        .query_strings(&format!(
            "select entity_id, record_key from {}",
            er_cli::ENTITY_MEMBERSHIP
        ))
        .unwrap()
        .into_iter()
        .map(|r| (r[0].clone(), r[1].clone()))
        .collect();
    partition(&rows)
}

fn membership_golden(scenario: &str, phase: &str) -> BTreeSet<BTreeSet<String>> {
    let csv = std::fs::read_to_string(repo_fixture(&format!(
        "{scenario}/expected/{phase}/membership.csv"
    )))
    .unwrap();
    let mut rows: Vec<(String, String)> = Vec::new();
    for line in csv.lines().skip(1).filter(|l| !l.is_empty()) {
        let f: Vec<&str> = line.split(',').collect();
        rows.push((f[3].to_string(), format!("{}:{}", f[1], f[2])));
    }
    partition(&rows)
}

/// Mirror of er-standardize's std_hashes gate column render.
fn render_col(column: &str) -> String {
    match column {
        "name_variants" => "array_to_string(name_variants, chr(31))".to_string(),
        "email_valid" | "phone_valid" | "birth_date" | "updated_at_source" => {
            format!("coalesce(cast({column} as varchar), '')")
        }
        _ => format!("coalesce({column}, '')"),
    }
}

#[test]
fn ingest_and_standardize_through_ducklake_match_golden() {
    if !enabled() {
        eprintln!("skipping DuckLake e2e (set DEDUPE_E2E_DUCKLAKE=1 to run)");
        return;
    }
    let (session, _config, tmp) = standardized_lake("std");

    let select_cols: Vec<String> = STD_HASH_COLUMNS.iter().map(|c| render_col(c)).collect();
    let rows = session
        .query_strings(&format!(
            "select source_system, source_record_id, {} from {}",
            select_cols.join(", "),
            er_cli::INT_STD_RECORDS
        ))
        .unwrap();
    let mut computed: BTreeMap<(String, String), String> = BTreeMap::new();
    for row in &rows {
        let mut vals: BTreeMap<String, StdValue> = BTreeMap::new();
        for (i, column) in STD_HASH_COLUMNS.iter().enumerate() {
            vals.insert((*column).to_string(), StdValue::Text(row[2 + i].clone()));
        }
        computed.insert(
            (row[0].clone(), row[1].clone()),
            table_content_hash(&vals).unwrap(),
        );
    }

    let golden =
        std::fs::read_to_string(repo_fixture("base_10/expected/base/std_hashes.csv")).unwrap();
    let mut expected: BTreeMap<(String, String), String> = BTreeMap::new();
    for line in golden.lines().skip(1).filter(|l| !l.is_empty()) {
        let f: Vec<&str> = line.split(',').collect();
        expected.insert((f[0].to_string(), f[1].to_string()), f[2].to_string());
    }

    assert_eq!(
        computed.len(),
        23,
        "expected 23 current std rows in the lake"
    );
    assert_eq!(
        computed, expected,
        "lake int_std_records std_hashes differ from the committed golden"
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn match_through_ducklake_matches_golden() {
    if !enabled() {
        eprintln!("skipping DuckLake e2e (set DEDUPE_E2E_DUCKLAKE=1 to run)");
        return;
    }
    let (session, _config, tmp) = standardized_lake("match");

    let model_json = std::fs::read_to_string(repo_fixture("model_test_v1.json")).unwrap();
    let tf_csv = repo_fixture("model_test_v1.tf.csv");
    er_cli::run_match(
        &session,
        &er_cli::MatchModel {
            settings_json: &model_json,
            tf_csv_path: std::path::Path::new(&tf_csv),
            model_version: "v0001",
            tf_snapshot_id: "01JWMDTFSNAP00000000000001",
        },
        0.60, // review_low
        "01RUNMATCH0000000000000000",
        "2026-01-02 00:00:00",
    )
    .unwrap();

    // Load the committed golden (rec_a_key, rec_b_key, match_probability) and
    // compare in SQL (DOUBLE vs DOUBLE) at 1e-10.
    let golden_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/match_scores_base_10.csv")
        .to_string_lossy()
        .replace('\'', "''");
    session
        .execute_batch(&format!(
            "create temp table golden as select * from read_csv('{golden_path}', header = true, \
             columns = {{'rec_a_key': 'VARCHAR', 'rec_b_key': 'VARCHAR', 'match_probability': 'DOUBLE'}})"
        ))
        .unwrap();

    let lake_n = session
        .query_strings(&format!("select count(*) from {}", er_cli::MATCH_SCORES))
        .unwrap()[0][0]
        .clone();
    assert_eq!(
        lake_n, "19",
        "expected 19 scored pairs in lake.match_scores"
    );

    let diff = session
        .count(&format!(
            "(
               (select rec_a_key, rec_b_key from {ms} except select rec_a_key, rec_b_key from golden)
               union all
               (select rec_a_key, rec_b_key from golden except select rec_a_key, rec_b_key from {ms})
             )",
            ms = er_cli::MATCH_SCORES,
        ))
        .unwrap();
    assert_eq!(
        diff, 0,
        "{diff} canonical pairs differ from the match golden"
    );

    let max_abs_diff = session
        .query_scalar_f64(&format!(
            "select coalesce(max(abs(m.match_probability - g.match_probability)), 0.0) \
             from {ms} m join golden g using (rec_a_key, rec_b_key)",
            ms = er_cli::MATCH_SCORES,
        ))
        .unwrap();
    assert!(
        max_abs_diff <= 1e-10,
        "max |match_probability - golden| = {max_abs_diff:e} exceeds 1e-10"
    );

    // The evidence waterfall is populated (NOT NULL) for every scored pair.
    let null_evidence = session
        .count(&format!(
            "(select 1 from {ms} where evidence is null)",
            ms = er_cli::MATCH_SCORES
        ))
        .unwrap();
    assert_eq!(null_evidence, 0, "every scored pair must carry evidence");

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn reconcile_through_ducklake_matches_golden() {
    if !enabled() {
        eprintln!("skipping DuckLake e2e (set DEDUPE_E2E_DUCKLAKE=1 to run)");
        return;
    }
    let (session, config, tmp) = standardized_lake("recon");
    match_model(&session);
    er_cli::run_reconcile(
        &session,
        &config,
        "01RUNRECON0000000000000000",
        "2026-01-03 00:00:00",
    )
    .unwrap();

    // Membership partition (id-insensitive) vs base_10 membership.csv.
    let got_rows: Vec<(String, String)> = session
        .query_strings(&format!(
            "select entity_id, record_key from {}",
            er_cli::ENTITY_MEMBERSHIP
        ))
        .unwrap()
        .into_iter()
        .map(|r| (r[0].clone(), r[1].clone()))
        .collect();
    let got = partition(&got_rows);

    let membership =
        std::fs::read_to_string(repo_fixture("base_10/expected/base/membership.csv")).unwrap();
    let mut want_rows: Vec<(String, String)> = Vec::new();
    for line in membership.lines().skip(1).filter(|l| !l.is_empty()) {
        // persona_id, source_system, source_record_id, entity_label
        let f: Vec<&str> = line.split(',').collect();
        want_rows.push((f[3].to_string(), format!("{}:{}", f[1], f[2])));
    }
    let want = partition(&want_rows);
    assert_eq!(got.len(), 10, "base_10 resolves to 10 entities in the lake");
    assert_eq!(
        got, want,
        "lake entity_membership partition differs from golden"
    );

    // Event profiles (per-entity {event_type: count}) vs events.csv.
    let mut got_events: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();
    for r in session
        .query_strings(&format!(
            "select entity_id, event_type from {}",
            er_cli::ENTITY_EVENTS
        ))
        .unwrap()
    {
        *got_events
            .entry(r[0].clone())
            .or_default()
            .entry(r[1].clone())
            .or_default() += 1;
    }
    let mut got_profiles: Vec<Vec<(String, usize)>> = got_events
        .into_values()
        .map(|m| m.into_iter().collect())
        .collect();
    got_profiles.sort();

    let events = std::fs::read_to_string(repo_fixture("base_10/expected/base/events.csv")).unwrap();
    let mut want_events: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();
    for line in events.lines().skip(1).filter(|l| !l.is_empty()) {
        // entity_label, event_type, count
        let f: Vec<&str> = line.split(',').collect();
        want_events
            .entry(f[0].to_string())
            .or_default()
            .insert(f[1].to_string(), f[2].parse().unwrap());
    }
    let mut want_profiles: Vec<Vec<(String, usize)>> = want_events
        .into_values()
        .map(|m| m.into_iter().collect())
        .collect();
    want_profiles.sort();

    assert_eq!(
        got_profiles, want_profiles,
        "lake entity_events differ from golden"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

/// golden.csv attribute columns (entity id dropped), in file order.
const GOLDEN_COLUMNS: [&str; 13] = [
    "given_name",
    "family_name",
    "email",
    "phone_e164",
    "addr_number",
    "addr_street",
    "addr_unit",
    "addr_city",
    "addr_region",
    "addr_postal",
    "birth_date",
    "metadata",
    "survivorship_version",
];

#[test]
fn golden_through_ducklake_matches_golden() {
    if !enabled() {
        eprintln!("skipping DuckLake e2e (set DEDUPE_E2E_DUCKLAKE=1 to run)");
        return;
    }
    let (session, config, tmp) = standardized_lake("golden");
    match_model(&session);
    er_cli::run_reconcile(
        &session,
        &config,
        "01RUNRECON0000000000000000",
        "2026-01-03 00:00:00",
    )
    .unwrap();
    er_cli::run_golden(&session, &config).unwrap();

    // golden_lineage by entity signature {attribute, record_key, rule}.
    let mut got_lineage: BTreeMap<String, BTreeSet<(String, String, String)>> = BTreeMap::new();
    for r in session
        .query_strings(&format!(
            "select entity_id, attribute, record_key, rule from {}",
            er_cli::GOLDEN_LINEAGE
        ))
        .unwrap()
    {
        got_lineage.entry(r[0].clone()).or_default().insert((
            r[1].clone(),
            r[2].clone(),
            r[3].clone(),
        ));
    }
    let got_lineage_set: BTreeSet<BTreeSet<(String, String, String)>> =
        got_lineage.into_values().collect();

    let lineage =
        std::fs::read_to_string(repo_fixture("base_10/expected/base/lineage.csv")).unwrap();
    let mut want_lineage: BTreeMap<String, BTreeSet<(String, String, String)>> = BTreeMap::new();
    for line in lineage.lines().skip(1).filter(|l| !l.is_empty()) {
        // entity_label, attribute, record_key, source_system, source_record_id, rule, ver
        let f: Vec<&str> = line.split(',').collect();
        want_lineage.entry(f[0].to_string()).or_default().insert((
            f[1].to_string(),
            f[2].to_string(),
            f[5].to_string(),
        ));
    }
    let want_lineage_set: BTreeSet<BTreeSet<(String, String, String)>> =
        want_lineage.into_values().collect();
    assert_eq!(
        got_lineage_set.len(),
        10,
        "golden_lineage covers 10 entities"
    );
    assert_eq!(
        got_lineage_set, want_lineage_set,
        "lake golden_lineage differs from golden"
    );

    // golden_records attribute columns (id dropped) vs golden.csv, in SQL (NULL/JSON).
    let golden_path = repo_fixture("base_10/expected/base/golden.csv").replace('\'', "''");
    let cols_decl = GOLDEN_COLUMNS
        .iter()
        .map(|c| format!("'{c}': 'VARCHAR'"))
        .collect::<Vec<_>>()
        .join(", ");
    session
        .execute_batch(&format!(
            "create temp table golden_expected as select * from read_csv('{golden_path}', \
             header = true, nullstr = '\\N', columns = {{'entity_label': 'VARCHAR', {cols_decl}}})"
        ))
        .unwrap();
    let projection = GOLDEN_COLUMNS
        .iter()
        .map(|c| format!("cast({c} as varchar) as {c}"))
        .collect::<Vec<_>>()
        .join(", ");
    let cols = GOLDEN_COLUMNS.join(", ");
    let diff = session
        .count(&format!(
            "((select {projection} from {gr} except select {cols} from golden_expected) \
              union all \
              (select {cols} from golden_expected except select {projection} from {gr}))",
            gr = er_cli::GOLDEN_RECORDS,
        ))
        .unwrap();
    assert_eq!(
        diff, 0,
        "{diff} lake golden_records rows differ from golden.csv"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn incremental_merge_through_ducklake_matches_golden() {
    if !enabled() {
        eprintln!("skipping DuckLake e2e (set DEDUPE_E2E_DUCKLAKE=1 to run)");
        return;
    }
    // run_reconcile reads prior entity_membership from the lake, so resolving a
    // second delivery against the first IS the incremental flow — no extra code.
    let (session, config, tmp) = open_seeded_lake("incmerge");

    let base = resolve_phase_through_lake(
        &session,
        &config,
        "merge_scenario",
        "base",
        "01BATCHMERGEBASE0000000000",
        "2026-01-01 00:00:00",
    );
    assert_eq!(
        base,
        membership_golden("merge_scenario", "base"),
        "base membership differs"
    );
    assert_eq!(base.len(), 2, "merge_scenario base resolves to 2 entities");

    let batch = resolve_phase_through_lake(
        &session,
        &config,
        "merge_scenario",
        "batch",
        "01BATCHMERGEBATCH000000000",
        "2026-06-01 12:00:00",
    );
    assert_eq!(
        batch,
        membership_golden("merge_scenario", "batch"),
        "batch membership differs"
    );
    assert_eq!(
        batch.len(),
        1,
        "the bridging record merges them into 1 entity"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn doctor_reports_an_empty_lake_as_not_ok() {
    if !enabled() {
        eprintln!("skipping DuckLake e2e (set DEDUPE_E2E_DUCKLAKE=1 to run)");
        return;
    }
    // A seeded-but-unrun lake has no pipeline relations yet.
    let (session, config, tmp) = open_seeded_lake("doctor_empty");
    let report = er_cli::doctor(&session, &config).unwrap();
    assert_eq!(report["ok"], false, "an unpopulated lake is not ok");
    assert_eq!(report["latest_run"], serde_json::Value::Null);
    assert!(
        report["relations"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["present"] == false),
        "no relations exist before the first run"
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

/// `entity_id -> record_key` membership, as a flat vec.
fn membership_rows(session: &Session) -> Vec<(String, String)> {
    session
        .query_strings(&format!(
            "select entity_id, record_key from {}",
            er_cli::ENTITY_MEMBERSHIP
        ))
        .unwrap()
        .into_iter()
        .map(|r| (r[0].clone(), r[1].clone()))
        .collect()
}

/// A bare lake with base_10 fully resolved via `run_all` (writes the `runs`
/// ledger row that `run_correct` reads the model identity from).
fn run_all_base(tag: &str) -> (Session, Config, PathBuf) {
    let tmp = std::env::temp_dir().join(format!("er_cli_{tag}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(tmp.join("data")).unwrap();
    let catalog = tmp.join("catalog.ducklake").to_string_lossy().into_owned();
    let data_path = tmp.join("data").to_string_lossy().into_owned();
    let session = er_cli::open_lake(&catalog, &data_path).unwrap();
    let config = load_config_str(include_str!("fixtures/test.yaml")).unwrap();

    let lexicon: Vec<(String, String)> = include_str!("fixtures/nickname_variants.csv")
        .lines()
        .skip(1)
        .filter(|l| !l.is_empty())
        .map(|l| {
            let (a, b) = l.split_once(',').unwrap();
            (a.to_string(), b.to_string())
        })
        .collect();
    let deliveries: Vec<(String, String)> = ["crm", "billing", "webforms"]
        .iter()
        .map(|s| {
            (
                (*s).to_string(),
                std::fs::read_to_string(repo_fixture(&format!("base_10/base/{s}.csv"))).unwrap(),
            )
        })
        .collect();
    let model_json = std::fs::read_to_string(repo_fixture("model_test_v1.json")).unwrap();
    let tf_csv = repo_fixture("model_test_v1.tf.csv");
    er_cli::run_all(
        &session,
        &config,
        &er_cli::PipelineInputs {
            lexicon: &lexicon,
            deliveries: &deliveries,
            model: &er_cli::MatchModel {
                settings_json: &model_json,
                tf_csv_path: std::path::Path::new(&tf_csv),
                model_version: "v0001",
                tf_snapshot_id: "01JWMDTFSNAP00000000000001",
            },
            run_id: "01RUNALLFULL0000000000000000",
            ingested_at: "2026-01-01 00:00:00",
            scored_at: "2026-01-02 00:00:00",
            run_at: "2026-01-03 00:00:00",
        },
    )
    .unwrap();
    (session, config, tmp)
}

#[test]
fn correct_applies_a_staged_assertion_without_rescoring() {
    if !enabled() {
        eprintln!("skipping DuckLake e2e (set DEDUPE_E2E_DUCKLAKE=1 to run)");
        return;
    }
    let (session, config, tmp) = run_all_base("correct");

    // Baseline: 10 entities, and the full run's ledger row.
    let base = membership_rows(&session);
    let base_entities: BTreeSet<String> = base.iter().map(|(e, _)| e.clone()).collect();
    assert_eq!(base_entities.len(), 10);

    // Pick two records sitting in different entities, stage an `always` between
    // them (as the steward drain would), then run `correct` — NO re-ingest/score.
    let mut by_entity: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (e, r) in &base {
        by_entity.entry(e.clone()).or_default().push(r.clone());
    }
    let ents: Vec<(String, Vec<String>)> = by_entity.into_iter().collect();
    let (rec_a, rec_b) = (ents[0].1[0].clone(), ents[1].1[0].clone());
    let action =
        serde_json::json!({ "type": "add_assertion", "kind": "always", "a": rec_a, "b": rec_b });
    er_cli::apply_staged_action(&session, "act-correct", &action, "2026-02-01 00:00:00").unwrap();

    er_cli::run_correct(
        &session,
        &config,
        "01CORRECT0000000000000000AA",
        "2026-02-02 00:00:00",
    )
    .unwrap();

    // The two entities merged (10 -> 9) via reconcile alone.
    let after = membership_rows(&session);
    let entity_of = |rec: &str| after.iter().find(|(_, r)| r == rec).unwrap().0.clone();
    assert_eq!(
        entity_of(&rec_a),
        entity_of(&rec_b),
        "always merged the pair"
    );
    let after_entities: BTreeSet<String> = after.iter().map(|(e, _)| e.clone()).collect();
    assert_eq!(after_entities.len(), 9);

    // The ledger carries both the full run and the correct pass.
    let modes: Vec<String> = session
        .query_strings("select mode from lake.main.runs")
        .unwrap()
        .into_iter()
        .map(|r| r[0].clone())
        .collect();
    assert!(modes.iter().any(|m| m == "full"));
    assert!(modes.iter().any(|m| m == "correct"), "correct run recorded");

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn train_produces_a_scorable_model() {
    if !enabled() {
        eprintln!("skipping DuckLake e2e (set DEDUPE_E2E_DUCKLAKE=1 to run)");
        return;
    }
    // base_10 standardized gives int_std_records to fit on. Training over 23
    // records is degenerate by design (S4.3.2) — this gate proves the training
    // PATH produces artifacts that load + score, not bit-parity params.
    let (session, config, tmp) = standardized_lake("train");

    let template = std::fs::read_to_string(repo_fixture("model_test_v1.json")).unwrap();
    let out_model = tmp.join("trained.json");
    let out_tf = tmp.join("trained.tf.csv");
    er_cli::run_train(
        &session,
        &er_cli::TrainInputs {
            template_settings_json: &template,
            deterministic_rules: &[r#"l."email" = r."email""#.to_string()],
            em_rules: &[r#"l."given_name" = r."given_name""#.to_string()],
            recall: 0.9,
            u_max_pairs: 1e7,
            model_version: "vtrain",
            tf_snapshot_id: "01TRAINSNAP0000000000000001",
        },
        &out_model,
        &out_tf,
    )
    .unwrap();
    assert!(out_model.exists(), "trained settings written");
    assert!(out_tf.exists(), "TF snapshot written");

    // The trained artifacts round-trip: a full match with them scores pairs.
    let trained = std::fs::read_to_string(&out_model).unwrap();
    er_cli::run_match(
        &session,
        &er_cli::MatchModel {
            settings_json: &trained,
            tf_csv_path: &out_tf,
            model_version: "vtrain",
            tf_snapshot_id: "01TRAINSNAP0000000000000001",
        },
        config.thresholds.review_low,
        "01TRAINRUN00000000000000001",
        "2026-01-02 00:00:00",
    )
    .unwrap();
    let scored = session.count(er_cli::MATCH_SCORES).unwrap();
    assert!(scored >= 1, "the trained model scored at least one pair");

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn assertions_enforce_clustering_through_ducklake() {
    if !enabled() {
        eprintln!("skipping DuckLake e2e (set DEDUPE_E2E_DUCKLAKE=1 to run)");
        return;
    }
    let (session, config, tmp) = standardized_lake("assert");
    match_model(&session);
    er_cli::run_reconcile(
        &session,
        &config,
        "01RUNBASE00000000000000000",
        "2026-01-03 00:00:00",
    )
    .unwrap();

    // Baseline: base_10 resolves to 10 entities.
    let base = membership_rows(&session);
    let base_entities: BTreeSet<String> = base.iter().map(|(e, _)| e.clone()).collect();
    assert_eq!(base_entities.len(), 10, "base_10 baseline is 10 entities");

    // --- `always`: merge two records from two different entities. ---
    let mut by_entity: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (e, r) in &base {
        by_entity.entry(e.clone()).or_default().push(r.clone());
    }
    let ents: Vec<(String, Vec<String>)> = by_entity.into_iter().collect();
    let rec_a = ents[0].1[0].clone();
    let rec_b = ents[1].1[0].clone();
    assert_ne!(
        ents[0].0, ents[1].0,
        "the two records start in different entities"
    );

    let action =
        serde_json::json!({ "type": "add_assertion", "kind": "always", "a": rec_a, "b": rec_b });
    er_cli::apply_staged_action(&session, "act-always", &action, "2026-01-04 00:00:00").unwrap();
    er_cli::run_reconcile(
        &session,
        &config,
        "01RUNALW000000000000000000",
        "2026-01-05 00:00:00",
    )
    .unwrap();

    let after = membership_rows(&session);
    let entity_of =
        |rec: &str| -> String { after.iter().find(|(_, r)| r == rec).unwrap().0.clone() };
    assert_eq!(
        entity_of(&rec_a),
        entity_of(&rec_b),
        "the always assertion co-clusters the two records"
    );
    let after_entities: BTreeSet<String> = after.iter().map(|(e, _)| e.clone()).collect();
    assert_eq!(after_entities.len(), 9, "two entities merged into one");

    // --- `never`: split two records that share an entity. ---
    let mut by_entity2: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (e, r) in &after {
        by_entity2.entry(e.clone()).or_default().push(r.clone());
    }
    let (_, members) = by_entity2
        .iter()
        .find(|(_, members)| members.len() >= 2)
        .expect("a multi-member entity to split");
    let (split_a, split_b) = (members[0].clone(), members[1].clone());

    let never =
        serde_json::json!({ "type": "add_assertion", "kind": "never", "a": split_a, "b": split_b });
    er_cli::apply_staged_action(&session, "act-never", &never, "2026-01-06 00:00:00").unwrap();
    er_cli::run_reconcile(
        &session,
        &config,
        "01RUNNVR000000000000000000",
        "2026-01-07 00:00:00",
    )
    .unwrap();

    let split = membership_rows(&session);
    let split_entity =
        |rec: &str| -> String { split.iter().find(|(_, r)| r == rec).unwrap().0.clone() };
    assert_ne!(
        split_entity(&split_a),
        split_entity(&split_b),
        "the never assertion separates the two records"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn preview_match_scores_without_committing() {
    if !enabled() {
        eprintln!("skipping DuckLake e2e (set DEDUPE_E2E_DUCKLAKE=1 to run)");
        return;
    }
    // Standardized only — run_match is deliberately NOT called, so match_scores
    // does not exist going in. A read-only preview must leave it that way.
    let (session, _config, tmp) = standardized_lake("preview");
    let snap_before = session.latest_snapshot_version().unwrap();

    let model_json = std::fs::read_to_string(repo_fixture("model_test_v1.json")).unwrap();
    let tf_csv = repo_fixture("model_test_v1.tf.csv");
    let preview = er_cli::preview_match(
        &session,
        &er_cli::MatchModel {
            settings_json: &model_json,
            tf_csv_path: std::path::Path::new(&tf_csv),
            model_version: "v0001",
            tf_snapshot_id: "01JWMDTFSNAP00000000000001",
        },
        0.60, // review_low
        0.90, // auto_merge
        200,  // sample_limit
    )
    .unwrap();

    // Same 19 canonical pairs run_match commits at review_low 0.60.
    assert_eq!(preview.total, 19, "preview scores the 19 base_10 pairs");
    assert_eq!(
        preview.would_auto_merge + preview.would_review,
        preview.total,
        "every scored pair buckets into auto-merge or review"
    );
    assert_eq!(preview.sample.len(), 19, "limit 200 returns all 19 pairs");
    assert!(
        preview
            .sample
            .iter()
            .all(|p| !p.evidence.is_empty() && p.evidence != "null"),
        "every sample pair carries an evidence waterfall"
    );
    assert!(
        preview
            .sample
            .windows(2)
            .all(|w| w[0].match_probability >= w[1].match_probability),
        "sample is ordered by descending probability"
    );

    // READ-ONLY: match_scores was never created, and no new lake snapshot was
    // committed — the preview touched nothing durable.
    assert!(
        session
            .count(&format!("(select 1 from {})", er_cli::MATCH_SCORES))
            .is_err(),
        "match_scores must not exist after a read-only preview"
    );
    assert_eq!(
        snap_before,
        session.latest_snapshot_version().unwrap(),
        "preview must not commit a new lake snapshot"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}
