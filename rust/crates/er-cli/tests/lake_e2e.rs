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
    let config = load_config_str(include_str!("fixtures/test.yaml")).unwrap();
    let model_json = std::fs::read_to_string(repo_fixture("model_test_v1.json")).unwrap();
    let tf_csv = repo_fixture("model_test_v1.tf.csv");
    er_cli::run_match(
        session,
        &config,
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
    er_cli::run_standardize(&session, &config, "2026-01-01 00:00:00").unwrap();
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
    er_cli::run_standardize(session, config, ts).unwrap();
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

/// A fresh lake with one scenario phase delivered and standardized, matched
/// through `run_all` in `mode` — the whole chain, so the mode dispatch in
/// `run_all` is what is under test rather than a stage called directly.
fn run_all_phase(
    session: &Session,
    config: &Config,
    scenario: &str,
    phase: &str,
    run_id: &str,
    ts: &str,
    mode: &str,
) {
    let deliveries: Vec<(String, String)> = ["crm", "billing", "webforms"]
        .iter()
        .filter_map(|s| {
            std::fs::read_to_string(repo_fixture(&format!("{scenario}/{phase}/{s}.csv")))
                .ok()
                .map(|csv| ((*s).to_string(), csv))
        })
        .collect();
    let model_json = std::fs::read_to_string(repo_fixture("model_test_v1.json")).unwrap();
    let tf_csv = repo_fixture("model_test_v1.tf.csv");
    er_cli::run_all(
        session,
        config,
        &er_cli::PipelineInputs {
            mode,
            rebuild_reason: None,
            segment: None,
            lexicon: &lexicon_pairs(),
            deliveries: &deliveries,
            model: &er_cli::MatchModel {
                settings_json: &model_json,
                tf_csv_path: std::path::Path::new(&tf_csv),
                model_version: "v0001",
                tf_snapshot_id: "01JWMDTFSNAP00000000000001",
            },
            run_id,
            ingested_at: ts,
            scored_at: ts,
            run_at: ts,
        },
    )
    .unwrap();
}

fn lexicon_pairs() -> Vec<(String, String)> {
    include_str!("fixtures/nickname_variants.csv")
        .lines()
        .skip(1)
        .filter(|l| !l.is_empty())
        .map(|l| {
            let (a, b) = l.split_once(',').unwrap();
            (a.to_string(), b.to_string())
        })
        .collect()
}

/// Every ACTIVE score as `(rec_a_key, rec_b_key) -> probability`, rendered as a
/// string so the comparison is bit-exact rather than within a tolerance.
fn active_scores(session: &Session) -> BTreeMap<(String, String), String> {
    session
        .query_strings(&format!(
            "select rec_a_key, rec_b_key, cast(match_probability as varchar)
             from {} where is_active order by rec_a_key, rec_b_key",
            er_cli::MATCH_SCORES
        ))
        .unwrap()
        .into_iter()
        .map(|r| ((r[0].clone(), r[1].clone()), r[2].clone()))
        .collect()
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
    let (session, config, tmp) = standardized_lake("match");

    let model_json = std::fs::read_to_string(repo_fixture("model_test_v1.json")).unwrap();
    let tf_csv = repo_fixture("model_test_v1.tf.csv");
    er_cli::run_match(
        &session,
        &config,
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
    er_cli::run_golden(&session, &config, "2026-01-03 00:00:00").unwrap();

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

/// PR 1.6b through the lake: a `combined_list` (Bucket B) survivorship chain
/// SYNTHESIZES an attribute's golden value by unioning every member's distinct
/// value, rather than electing one record's. Exercises the records-assembly
/// correlated-aggregate path end to end (the decision + multi-value correctness
/// are unit-tested in er-golden). base_10 is resolved, then `email`'s chain is
/// overridden to `combined_list` and golden re-assembled; the golden email of the
/// largest entity must equal its members' sorted distinct non-empty emails.
#[test]
fn combined_list_survivorship_synthesizes_golden_value_through_ducklake() {
    if !enabled() {
        eprintln!("skipping DuckLake e2e (set DEDUPE_E2E_DUCKLAKE=1 to run)");
        return;
    }
    let (session, config, tmp) = standardized_lake("combinedlist");
    match_model(&session);
    er_cli::run_reconcile(
        &session,
        &config,
        "01RUNCL0000000000000000000",
        "2026-01-03 00:00:00",
    )
    .unwrap();

    // Override email survivorship to the Bucket B combined_list rule, re-assemble.
    let mut cfg = config.clone();
    cfg.survivorship
        .insert("email".to_string(), vec!["combined_list".to_string()]);
    er_cli::run_golden(&session, &cfg, "2026-01-03 00:00:00").unwrap();

    // The entity with the most distinct non-empty member emails (base_10's E1 has
    // four members) — guarantees the union path is exercised.
    let pick = session
        .query_strings(&format!(
            "select m.entity_id from {std} r \
               join {mem} m on m.record_key = r.record_key \
             where r.email is not null and r.email <> '' \
             group by m.entity_id \
             order by count(distinct r.email) desc, m.entity_id asc limit 1",
            std = er_cli::INT_STD_RECORDS,
            mem = er_cli::ENTITY_MEMBERSHIP,
        ))
        .unwrap();
    let entity = pick[0][0].clone();

    let expected: String = session
        .query_strings(&format!(
            "select distinct r.email from {std} r \
               join {mem} m on m.record_key = r.record_key \
             where m.entity_id = '{entity}' and r.email is not null and r.email <> '' \
             order by r.email",
            std = er_cli::INT_STD_RECORDS,
            mem = er_cli::ENTITY_MEMBERSHIP,
        ))
        .unwrap()
        .iter()
        .map(|r| r[0].clone())
        .collect::<Vec<_>>()
        .join("; ");
    assert!(
        !expected.is_empty(),
        "the chosen entity should carry an email"
    );

    let got = session
        .query_strings(&format!(
            "select email from {gr} where id = '{entity}'",
            gr = er_cli::GOLDEN_RECORDS,
        ))
        .unwrap();
    assert_eq!(got.len(), 1, "one golden row for the entity");
    assert_eq!(
        got[0][0], expected,
        "combined_list golden email should union the members' distinct emails"
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

/// THE equivalence gate: an incremental batch must land the corpus in exactly
/// the state a full re-run over base ∪ batch would.
///
/// This is DesignDoc §4.5.6's INV-EQ and T-INC-3, and it is the only thing that
/// makes incremental mode safe to default to. The `incremental_batch` fixture
/// was committed for this test and had no consumer: its six batch records carry
/// declared roles (`attribution.csv`) — three join an existing entity, two form
/// a new entity between themselves, one bridges two existing entities — chosen
/// so that an implementation can fail each way distinguishably.
///
/// Two universes, same inputs, different route: A ingests the batch and scores
/// it incrementally; B scores the union from scratch. Both the active scores
/// (bit-exact probabilities, not a tolerance) and the resulting partition must
/// agree, and the partition must equal the fixture's ground truth so that "the
/// two paths agree" cannot be satisfied by both being wrong.
#[test]
fn an_incremental_batch_lands_where_a_full_rerun_would() {
    if !enabled() {
        eprintln!("skipping DuckLake e2e (set DEDUPE_E2E_DUCKLAKE=1 to run)");
        return;
    }
    let truth = membership_golden("incremental_batch", "batch");

    // Universe A — base, then the batch INCREMENTALLY.
    let (sa, config, tmp_a) = open_seeded_lake("inc_eq_a");
    run_all_phase(
        &sa,
        &config,
        "incremental_batch",
        "base",
        "01INCEQA000000000000BASE000",
        "2026-01-01 00:00:00",
        "full",
    );
    run_all_phase(
        &sa,
        &config,
        "incremental_batch",
        "batch",
        "01INCEQA00000000000BATCH000",
        "2026-06-01 00:00:00",
        "incremental",
    );
    let scores_a = active_scores(&sa);
    let partition_a = partition(&membership_rows(&sa));

    // Universe B — the same corpus, scored whole.
    let (sb, _, tmp_b) = open_seeded_lake("inc_eq_b");
    run_all_phase(
        &sb,
        &config,
        "incremental_batch",
        "base",
        "01INCEQB000000000000BASE000",
        "2026-01-01 00:00:00",
        "full",
    );
    run_all_phase(
        &sb,
        &config,
        "incremental_batch",
        "batch",
        "01INCEQB00000000000BATCH000",
        "2026-06-01 00:00:00",
        "full",
    );
    let scores_b = active_scores(&sb);
    let partition_b = partition(&membership_rows(&sb));

    assert!(
        !scores_b.is_empty(),
        "precondition: the full run scored something"
    );
    // Compare the pair SETS first: a missing pair and a mis-scored pair are
    // different bugs and the key-set diff names which one happened.
    let keys_a: BTreeSet<_> = scores_a.keys().collect();
    let keys_b: BTreeSet<_> = scores_b.keys().collect();
    assert_eq!(
        keys_a,
        keys_b,
        "incremental and full disagree on WHICH pairs are scored; \
         only-incremental={:?} only-full={:?}",
        keys_a.difference(&keys_b).collect::<Vec<_>>(),
        keys_b.difference(&keys_a).collect::<Vec<_>>(),
    );
    assert_eq!(
        scores_a, scores_b,
        "incremental and full agree on the pairs but not their probabilities"
    );
    assert_eq!(
        partition_a, partition_b,
        "incremental and full produce different entities"
    );
    assert_eq!(
        partition_a, truth,
        "both paths agree, but neither matches the fixture's ground truth"
    );

    let _ = std::fs::remove_dir_all(&tmp_a);
    let _ = std::fs::remove_dir_all(&tmp_b);
}

/// Resolving the corpus one segment at a time must land exactly where one full
/// run would (#54).
///
/// This is the answer to "my corpus is too big for one run", and the whole
/// question is whether splitting the WORK can avoid splitting the DATA. The
/// fixture makes the difference impossible to fake: base_10's duplicates are all
/// **cross-source** by construction, so segmenting on `source_system` means
/// every single duplicate spans two segments. An implementation that filtered
/// the corpus to the segment — the obvious reading of "scope a run" — would find
/// no duplicates at all and this test would collapse.
///
/// Run with no new data on purpose. Each pass therefore has to invalidate its
/// slice's scores and rebuild them from the whole corpus, so the test is an
/// exact round-trip: three passes over a corpus that has not changed must leave
/// `match_scores` bit-identical to what the full run wrote.
#[test]
fn resolving_each_segment_in_turn_lands_where_one_full_run_would() {
    if !enabled() {
        eprintln!("skipping DuckLake e2e (set DEDUPE_E2E_DUCKLAKE=1 to run)");
        return;
    }
    let (session, mut config, tmp) = open_seeded_lake("seg_cover");
    // Declared in code rather than in the committed fixture so `config_hash`
    // and the config golden stay untouched; the YAML parse path has its own
    // test in er-config.
    for source in ["crm", "billing", "webforms"] {
        config
            .segments
            .insert(source.to_string(), format!("source_system = '{source}'"));
    }

    run_all_phase(
        &session,
        &config,
        "incremental_batch",
        "base",
        "01SEGCOVER0000000000BASE000",
        "2026-01-01 00:00:00",
        "full",
    );
    let full_scores = active_scores(&session);
    let full_partition = partition(&membership_rows(&session));

    // The premise, in both directions. Most base_10 duplicates span two sources
    // — so an implementation that filtered the corpus down to the segment would
    // lose them, and this test would collapse rather than pass. A couple are
    // within one source, so the within-slice pass is exercised too. Asserting
    // both keeps the test honest if the fixture ever changes shape.
    let same_source: Vec<_> = full_scores
        .keys()
        .filter(|(a, b)| a.split(':').next() == b.split(':').next())
        .collect();
    let cross_source = full_scores.len() - same_source.len();
    assert!(
        cross_source >= 15,
        "premise: most pairs must cross a segment boundary (found {cross_source} \
         of {})",
        full_scores.len()
    );
    assert!(
        !same_source.is_empty(),
        "premise: at least one pair must sit inside one segment, so the \
         within-slice pass is covered too"
    );

    let model_json = std::fs::read_to_string(repo_fixture("model_test_v1.json")).unwrap();
    let tf_csv = repo_fixture("model_test_v1.tf.csv");
    for (i, source) in ["crm", "billing", "webforms"].iter().enumerate() {
        let delta = er_cli::run_match_segment(
            &session,
            &config,
            &er_cli::MatchModel {
                settings_json: &model_json,
                tf_csv_path: std::path::Path::new(&tf_csv),
                model_version: "v0001",
                tf_snapshot_id: "01JWMDTFSNAP00000000000001",
            },
            config.thresholds.review_low,
            &format!("01SEGCOVER00000000000SEG{i:03}"),
            "2026-03-01 00:00:00",
            &config.segments[*source],
        )
        .unwrap();
        assert_eq!(delta.mode, "incremental", "{:?}", delta.full_reason);
        assert_eq!(
            delta.scope_predicate.as_deref(),
            Some(config.segments[*source].as_str()),
            "the pass should report the scope it ran under"
        );
        // The slice really was the unit of work: fewer records than the corpus,
        // and more than none.
        assert!(
            delta.delta_records > 0 && delta.delta_records < delta.corpus_records,
            "segment {source} scored {}/{} records",
            delta.delta_records,
            delta.corpus_records
        );
        assert!(
            delta.invalidated_scores > 0,
            "segment {source} should have superseded its own slice's scores"
        );
    }

    er_cli::run_reconcile(
        &session,
        &config,
        "01SEGCOVER0000000000RECON00",
        "2026-03-01 00:00:00",
    )
    .unwrap();

    let segmented = active_scores(&session);
    let keys_full: BTreeSet<_> = full_scores.keys().collect();
    let keys_seg: BTreeSet<_> = segmented.keys().collect();
    assert_eq!(
        keys_seg,
        keys_full,
        "covering the corpus by segment lost or invented pairs; \
         only-segmented={:?} only-full={:?}",
        keys_seg.difference(&keys_full).collect::<Vec<_>>(),
        keys_full.difference(&keys_seg).collect::<Vec<_>>(),
    );
    assert_eq!(
        segmented, full_scores,
        "the same pairs, but scored differently"
    );
    assert_eq!(
        partition(&membership_rows(&session)),
        full_partition,
        "segment-by-segment resolution produced different entities"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

/// The incremental pass must be CHEAPER, and both of its passes must be live.
///
/// Equivalence alone is satisfiable by a pass that quietly re-scores everything,
/// which is the most likely way for this feature to rot: it would stay green
/// forever while delivering nothing. So assert the delta directly — six records
/// scored out of twenty-nine — and assert that the new-vs-new pass found the
/// pair the fixture built for it. `attribution.csv` names which records that is,
/// and names the pass that MUST discover each, so the fixture decides what this
/// test asserts rather than the implementation.
#[test]
fn an_incremental_pass_scores_only_the_batch_and_still_finds_new_vs_new_pairs() {
    if !enabled() {
        eprintln!("skipping DuckLake e2e (set DEDUPE_E2E_DUCKLAKE=1 to run)");
        return;
    }
    let (session, config, tmp) = open_seeded_lake("inc_delta");
    run_all_phase(
        &session,
        &config,
        "incremental_batch",
        "base",
        "01INCDELTA0000000000BASE000",
        "2026-01-01 00:00:00",
        "full",
    );

    // Deliver the batch, then run the match pass directly so its report is
    // observable (run_all logs it; a test needs the value).
    for source in ["crm", "billing", "webforms"] {
        if let Ok(csv) = std::fs::read_to_string(repo_fixture(&format!(
            "incremental_batch/batch/{source}.csv"
        ))) {
            er_cli::run_ingest(
                &session,
                &config,
                source,
                &csv,
                "01INCDELTA000000000BATCH000",
                "2026-06-01 00:00:00",
                false,
            )
            .unwrap();
        }
    }
    er_cli::run_standardize(&session, &config, "2026-06-01 00:00:00").unwrap();
    let model_json = std::fs::read_to_string(repo_fixture("model_test_v1.json")).unwrap();
    let tf_csv = repo_fixture("model_test_v1.tf.csv");
    let delta = er_cli::run_match_incremental(
        &session,
        &config,
        &er_cli::MatchModel {
            settings_json: &model_json,
            tf_csv_path: std::path::Path::new(&tf_csv),
            model_version: "v0001",
            tf_snapshot_id: "01JWMDTFSNAP00000000000001",
        },
        config.thresholds.review_low,
        "01INCDELTA000000000BATCH000",
        "2026-06-01 00:00:00",
    )
    .unwrap();

    let attribution = std::fs::read_to_string(repo_fixture("incremental_batch/attribution.csv"))
        .expect("the fixture declares each batch record's role and pass");
    let batch_keys: Vec<&str> = attribution
        .lines()
        .skip(1)
        .filter(|l| !l.is_empty())
        .map(|l| l.split(',').next().unwrap())
        .collect();

    assert_eq!(delta.mode, "incremental", "{:?}", delta.full_reason);
    assert_eq!(
        delta.delta_records,
        batch_keys.len(),
        "exactly the batch should have been scored, not the corpus"
    );
    assert!(
        delta.corpus_records > delta.delta_records * 2,
        "the corpus ({}) should dwarf the delta ({}) or this proves nothing",
        delta.corpus_records,
        delta.delta_records
    );

    // Pass 2 coverage: the records the fixture marks `pass2` match each other
    // and nothing in the base corpus, so they are reachable ONLY by scoring the
    // delta against itself. If pass 2 were dropped, everything above would still
    // pass and these two records would silently become two entities.
    let pass2: Vec<&str> = attribution
        .lines()
        .skip(1)
        .filter(|l| !l.is_empty())
        .filter(|l| l.ends_with(",pass2"))
        .map(|l| l.split(',').next().unwrap())
        .collect();
    assert_eq!(pass2.len(), 2, "the fixture declares one new-vs-new pair");
    let scored = active_scores(&session);
    let (a, b) = (pass2[0].min(pass2[1]), pass2[0].max(pass2[1]));
    assert!(
        scored.contains_key(&(a.to_string(), b.to_string())),
        "the new-vs-new pair ({a}, {b}) that attribution.csv marks `pass2` was \
         never scored — predict_within over the delta is not running. Scored \
         pairs touching them: {:?}",
        scored
            .keys()
            .filter(|(x, y)| [a, b].contains(&x.as_str()) || [a, b].contains(&y.as_str()))
            .collect::<Vec<_>>()
    );

    er_cli::run_reconcile(
        &session,
        &config,
        "01INCDELTA000000000BATCH000",
        "2026-06-01 00:00:00",
    )
    .unwrap();
    let partition_now = partition(&membership_rows(&session));
    assert!(
        partition_now
            .iter()
            .any(|c| c.contains(&a.to_string()) && c.contains(&b.to_string())),
        "the new-vs-new pair scored but did not end up in one entity"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

/// A deleted record must leave its entity — and before this it could not.
///
/// `is_active` on `match_scores` was written `true` and never `false` by
/// anything in the workspace. Standardize drops a tombstoned record from the
/// corpus, but `run_reconcile` rebuilds its node universe from the node list
/// PLUS every surviving edge endpoint (its own comment says so, about
/// suppressions), so the deleted record's still-active scores dragged it back
/// into its old entity on every subsequent run. Deletion did not delete.
///
/// Both passes now deactivate what they are about to supersede before they
/// re-score, which is what makes `is_active` mean "the model currently says
/// this" rather than "the model said this once".
#[test]
fn a_deleted_record_stops_being_resurrected_by_its_own_stale_scores() {
    if !enabled() {
        eprintln!("skipping DuckLake e2e (set DEDUPE_E2E_DUCKLAKE=1 to run)");
        return;
    }
    let (session, config, tmp) = open_seeded_lake("inc_delete");
    run_all_phase(
        &session,
        &config,
        "incremental_batch",
        "base",
        "01INCDEL0000000000000BASE00",
        "2026-01-01 00:00:00",
        "full",
    );

    // Pick a record that shares an entity with others, so its removal is
    // observable as the entity shrinking rather than disappearing.
    let before = partition(&membership_rows(&session));
    let victim = before
        .iter()
        .find(|c| c.len() >= 2)
        .map(|c| c.iter().next().unwrap().clone())
        .expect("the base corpus has a multi-record entity");
    let (source, record_id) = victim.split_once(':').unwrap();

    // A full refresh of that source WITHOUT the victim row tombstones it, which
    // is how deletion reaches the lake (S4.1.1).
    let csv = std::fs::read_to_string(repo_fixture(&format!(
        "incremental_batch/base/{source}.csv"
    )))
    .unwrap();
    let mut kept: Vec<&str> = csv.lines().take(1).collect();
    kept.extend(
        csv.lines()
            .skip(1)
            .filter(|l| !l.is_empty() && !l.starts_with(&format!("{record_id},"))),
    );
    assert_eq!(
        kept.len(),
        csv.lines().filter(|l| !l.is_empty()).count() - 1,
        "the victim row must actually have been removed from the delivery"
    );
    er_cli::run_ingest(
        &session,
        &config,
        source,
        &format!("{}\n", kept.join("\n")),
        "01INCDEL000000000000DEL000",
        "2026-06-01 00:00:00",
        true,
    )
    .unwrap();
    er_cli::run_standardize(&session, &config, "2026-06-01 00:00:00").unwrap();
    // Precondition: the tombstone really did remove it from the matched corpus.
    // Without this, a failure below cannot be told apart from "the delivery was
    // never tombstoned in the first place".
    let corpus: BTreeSet<String> = session
        .query_strings(&format!(
            "select record_key from {}",
            er_cli::INT_STD_RECORDS
        ))
        .unwrap()
        .into_iter()
        .map(|r| r[0].clone())
        .collect();
    assert!(
        !corpus.contains(&victim),
        "precondition: the tombstoned record {victim} should be out of the \
         standardized corpus"
    );

    let model_json = std::fs::read_to_string(repo_fixture("model_test_v1.json")).unwrap();
    let tf_csv = repo_fixture("model_test_v1.tf.csv");
    let delta = er_cli::run_match_incremental(
        &session,
        &config,
        &er_cli::MatchModel {
            settings_json: &model_json,
            tf_csv_path: std::path::Path::new(&tf_csv),
            model_version: "v0001",
            tf_snapshot_id: "01JWMDTFSNAP00000000000001",
        },
        config.thresholds.review_low,
        "01INCDEL000000000000DEL000",
        "2026-06-01 00:00:00",
    )
    .unwrap();
    assert!(
        delta.invalidated_scores > 0,
        "the deleted record's scores should have been invalidated"
    );
    er_cli::run_reconcile(
        &session,
        &config,
        "01INCDEL000000000000DEL000",
        "2026-06-01 00:00:00",
    )
    .unwrap();

    let after = partition(&membership_rows(&session));
    assert!(
        !after.iter().any(|c| c.contains(&victim)),
        "the deleted record {victim} is still a member of an entity: {after:?}"
    );
    // And its former partners are still clustered together, so the invalidation
    // removed the deleted record's edges and not every edge it touched.
    let partners: BTreeSet<String> = before
        .iter()
        .find(|c| c.contains(&victim))
        .unwrap()
        .iter()
        .filter(|k| **k != victim)
        .cloned()
        .collect();
    assert!(
        after.iter().any(|c| partners.is_subset(c)),
        "deleting {victim} also broke up its entity's surviving members \
         {partners:?}: {after:?}"
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
            mode: "full",
            rebuild_reason: None,
            segment: None,
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

/// Resolving an unsaved record must find the entity it belongs to, and write
/// nothing while doing it.
///
/// This is the online "resolve this record" capability — the thing that lets a
/// form or an ingestion pipeline ask "who is this?" *before* saving. Two
/// properties decide whether it is usable:
///
/// 1. It agrees with the batch path. A record that is obviously Robert Chen
///    must come back pointing at the entity the full run put Robert Chen in,
///    with a band the caller can branch on. If the online answer and the batch
///    answer can differ, neither can be trusted.
/// 2. It commits nothing. A read-only question must not mutate the lake, or the
///    act of asking changes the corpus.
#[test]
fn resolving_an_unsaved_record_finds_its_entity_and_writes_nothing() {
    if !enabled() {
        eprintln!("skipping DuckLake e2e (set DEDUPE_E2E_DUCKLAKE=1 to run)");
        return;
    }
    let (session, config, tmp) = run_all_base("resolve");
    let model_json = std::fs::read_to_string(repo_fixture("model_test_v1.json")).unwrap();
    let tf_csv = repo_fixture("model_test_v1.tf.csv");
    let model = er_cli::MatchModel {
        settings_json: &model_json,
        tf_csv_path: std::path::Path::new(&tf_csv),
        model_version: "v0001",
        tf_snapshot_id: "01JWMDTFSNAP00000000000001",
    };
    let lexicon: Vec<(String, String)> = include_str!("fixtures/nickname_variants.csv")
        .lines()
        .skip(1)
        .filter(|l| !l.is_empty())
        .map(|l| {
            let (a, b) = l.split_once(',').unwrap();
            (a.to_string(), b.to_string())
        })
        .collect();

    // What the batch run decided about the record we are about to impersonate.
    let before = partition(&membership_rows(&session));
    let chen_entity: BTreeSet<String> = before
        .iter()
        .find(|c| c.contains("crm:C001"))
        .expect("base_10 resolves crm:C001 into an entity")
        .clone();
    assert!(
        chen_entity.len() > 1,
        "precondition: crm:C001 is in a multi-record entity"
    );

    let snapshot_before = session.latest_snapshot_version().unwrap();

    // A record that is plainly the same person as crm:C001 — the CRM source's
    // own column names, because the payload goes through the declared mapping.
    let payload = serde_json::json!({
        "crm_id": "LIVE-1",
        "first_name": "Robert",
        "last_name": "Chen",
        "email_address": "robert.chen@example.com",
        "phone": "(628) 555-0101",
        "street_address": "1420 Judah Street Apt 5",
        "city": "San Francisco",
        "state": "CA",
        "zip": "94107",
        "dob": "1985-03-14",
        "last_modified": "2026-01-01 00:00:00",
    });
    let result =
        er_cli::resolve_record(&session, &config, &model, "crm", &payload, &lexicon, 10).unwrap();

    assert_ne!(
        result.candidates.len(),
        0,
        "an obvious duplicate must produce candidates"
    );
    assert_eq!(
        result.decision,
        "auto_merge",
        "a record identical to an existing one should clear the auto-merge band, got {:?} at {:?}",
        result.decision,
        result.candidates.first().map(|c| c.match_probability)
    );

    // It agrees with the batch path: the top candidate belongs to the entity the
    // full run built for Robert Chen.
    let top = &result.candidates[0];
    assert!(
        chen_entity.contains(&top.record_key),
        "the top candidate ({}) should be a member of the Chen entity {chen_entity:?}",
        top.record_key
    );
    let entity_id = top
        .entity_id
        .as_deref()
        .expect("a reconciled record reports its entity");
    assert_ne!(entity_id, "");
    // Every candidate that is in the Chen entity reports the SAME entity id —
    // the caller is told one answer, not one per matching record.
    let ids: BTreeSet<&str> = result
        .candidates
        .iter()
        .filter(|c| chen_entity.contains(&c.record_key))
        .filter_map(|c| c.entity_id.as_deref())
        .collect();
    assert_eq!(
        ids.len(),
        1,
        "candidates inside one entity must all report that entity: {ids:?}"
    );
    // The evidence waterfall comes back, so a caller can explain its decision.
    assert!(
        top.evidence.is_object(),
        "the per-comparison evidence must be returned: {:?}",
        top.evidence
    );

    // Candidates are ordered best-first, which is what makes `[0]` meaningful.
    let probabilities: Vec<f64> = result
        .candidates
        .iter()
        .map(|c| c.match_probability)
        .collect();
    let mut sorted = probabilities.clone();
    sorted.sort_by(|a, b| b.partial_cmp(a).unwrap());
    assert_eq!(probabilities, sorted, "candidates must be best-first");

    // NOTHING WAS WRITTEN. The partition, the scored pairs and the snapshot are
    // all untouched, and no probe row leaked into the corpus.
    assert_eq!(
        partition(&membership_rows(&session)),
        before,
        "resolving must not change the partition"
    );
    assert_eq!(
        session.latest_snapshot_version().unwrap(),
        snapshot_before,
        "resolving must not advance the lake snapshot"
    );
    let probe_rows: i64 = session
        .query_strings(&format!(
            "select count(*) from {} where source_record_id like '%resolve%' \
             or source_record_id = 'LIVE-1'",
            er_cli::RAW_RECORDS
        ))
        .unwrap()[0][0]
        .parse()
        .unwrap();
    assert_eq!(probe_rows, 0, "the probe record must not reach raw_records");

    // A record that is nobody gets an honest rejection rather than a weak match.
    let stranger = serde_json::json!({
        "crm_id": "LIVE-2",
        "first_name": "Ingeborg",
        "last_name": "Zyzzyva",
        "email_address": "ingeborg.zyzzyva@elsewhere.example",
        "phone": "(907) 555-7788",
        "street_address": "9912 Tundra Road",
        "city": "Nome",
        "state": "AK",
        "zip": "99762",
        "dob": "1943-11-02",
        "last_modified": "2026-01-01 00:00:00",
    });
    let miss =
        er_cli::resolve_record(&session, &config, &model, "crm", &stranger, &lexicon, 10).unwrap();
    assert_eq!(
        miss.decision, "reject",
        "a record matching nobody must be rejected, not weakly matched: {:?}",
        miss.candidates
    );

    // An undeclared source is refused, naming the ones that exist.
    let err = er_cli::resolve_record(&session, &config, &model, "nosuch", &payload, &lexicon, 10)
        .expect_err("an undeclared source cannot be resolved");
    let message = format!("{err:#}");
    assert!(
        message.contains("crm"),
        "the refusal should name the declared sources: {message}"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

/// The blocking estimate must agree with what a run actually generates.
///
/// This is the answer to the competitor's hard three-hour analysis cap, whose
/// documented workaround is "split your dataset with filters or raise the
/// confidence floor" — two guesses made blind. An estimate is only worth having
/// if it is the same number the run works through, so this asserts exactly
/// that: the cumulative candidate count equals the number of distinct pairs the
/// pipeline scores at threshold 0 over the same corpus.
#[test]
fn the_blocking_estimate_matches_what_a_run_actually_scores() {
    if !enabled() {
        eprintln!("skipping DuckLake e2e (set DEDUPE_E2E_DUCKLAKE=1 to run)");
        return;
    }
    let (session, config, tmp) = standardized_lake("blocking_estimate");
    let model_json = std::fs::read_to_string(repo_fixture("model_test_v1.json")).unwrap();
    let rules: Vec<String> = serde_json::from_str::<serde_json::Value>(&model_json).unwrap()
        ["blocking_rules_to_generate_predictions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            r.as_str().map_or_else(
                || r["blocking_rule"].as_str().unwrap().to_string(),
                str::to_string,
            )
        })
        .collect();
    assert_eq!(
        rules.len(),
        4,
        "base_10's model declares four blocking rules"
    );

    let estimate = er_cli::estimate_blocking(&session, &model_json, &rules, 3).unwrap();

    // Blocking is a reduction: it must be cheaper than comparing everything.
    assert!(
        estimate.candidate_pairs < estimate.total_possible_pairs,
        "blocking must generate fewer pairs ({}) than the cartesian product ({})",
        estimate.candidate_pairs,
        estimate.total_possible_pairs
    );
    assert_ne!(estimate.candidate_pairs, 0, "and more than none");

    // Per-rule accounting: marginal counts sum to the cumulative total, and the
    // cumulative figure only ever grows.
    assert_eq!(estimate.per_rule.len(), rules.len());
    let marginal_sum: i64 = estimate.per_rule.iter().map(|r| r.marginal_pairs).sum();
    assert_eq!(
        marginal_sum, estimate.candidate_pairs,
        "the marginal counts must account for every candidate pair"
    );
    let mut previous = 0i64;
    for rule in &estimate.per_rule {
        // Non-DECREASING, not increasing: a rule may legitimately add nothing,
        // and saying so is the most actionable thing this estimate produces.
        assert!(
            rule.cumulative_pairs >= previous,
            "cumulative counts must be non-decreasing: {:?}",
            estimate.per_rule
        );
        assert!(rule.marginal_pairs >= 0);
        previous = rule.cumulative_pairs;
    }

    // base_10's own model carries a REDUNDANT rule, and the estimate finds it:
    // `birth_date || given_name` generates nothing the first three rules did not
    // already generate. That is precisely what a tenant needs told — it is
    // paying for a blocking pass that contributes no candidates — and it is
    // invisible without this breakdown. If a corpus change ever makes this rule
    // useful, this assertion is the thing that notices.
    let redundant: Vec<&str> = estimate
        .per_rule
        .iter()
        .filter(|r| r.marginal_pairs == 0)
        .map(|r| r.blocking_rule.as_str())
        .collect();
    assert_eq!(
        redundant.len(),
        1,
        "expected exactly one redundant rule over base_10: {:?}",
        estimate.per_rule
    );
    assert!(
        redundant[0].contains("birth_date"),
        "the redundant rule should be the dob/given-name one: {redundant:?}"
    );

    // THE CLAIM: the estimate is the number the run works through. Scoring with
    // a zero threshold keeps every candidate pair the blocking produced, so the
    // count of distinct scored pairs is exactly what was estimated.
    er_cli::run_match(
        &session,
        &config,
        &er_cli::MatchModel {
            settings_json: &model_json,
            tf_csv_path: std::path::Path::new(&repo_fixture("model_test_v1.tf.csv")),
            model_version: "v0001",
            tf_snapshot_id: "01JWMDTFSNAP00000000000001",
        },
        0.0,
        "01RUNESTIMATE000000000000000",
        "2026-01-02 00:00:00",
    )
    .unwrap();
    let scored: i64 = session
        .query_strings(&format!(
            "select count(*) from {} where rec_a_key <> rec_b_key",
            er_cli::MATCH_SCORES
        ))
        .unwrap()[0][0]
        .parse()
        .unwrap();
    assert_eq!(
        estimate.candidate_pairs, scored,
        "the estimate ({}) must equal the pairs the run actually scored ({scored}) — an \
         estimate that does not match the work is worse than none",
        estimate.candidate_pairs
    );

    // The largest-block list names the keys that would dominate a run. base_10
    // is small, so the point is the shape: at most what was asked for, each
    // with a positive pair count, biggest first.
    assert!(estimate.largest_blocks.len() <= 3);
    let mut last = i64::MAX;
    for block in &estimate.largest_blocks {
        assert!(block.pairs > 0, "a reported block must contain pairs");
        assert!(
            block.pairs <= last,
            "largest blocks must come biggest-first"
        );
        last = block.pairs;
    }

    let _ = std::fs::remove_dir_all(&tmp);
}

/// A hard constraint must actually keep a pair out of scoring — and this test
/// also pins down exactly how far that goes.
///
/// base_10's first golden cluster is four records for one person (Robert /
/// Bobby / Bob Chen) that agree on phone, address and date of birth and differ
/// only in email. Declaring `veto: [email]` says "two different non-blank
/// emails are proof these are different people", and the conjunct it compiles
/// into every blocking rule means such a pair is **never generated as a
/// candidate and never scored**. That is what this asserts, by looking for the
/// pair in `match_scores`.
///
/// What it does NOT do, and the second half of this test proves it rather than
/// leaving it to be discovered: candidate-level enforcement does not survive
/// transitive closure. C001 and C009 are never compared to each other, but both
/// are compared to W001 — whose email is a `test@test.com` placeholder that
/// normalizes to NULL, so no veto applies to either pairing — and connected
/// components then puts all three in one entity. Keeping vetoed records in
/// different ENTITIES needs the never-rule path at reconcile (the `never_cut`
/// fixpoint this repo already has for steward assertions), which is a separate
/// piece of work. Until then this is a candidate constraint, and the test says
/// so out loud.
#[test]
fn a_declared_veto_keeps_a_pair_out_of_scoring_but_not_out_of_a_transitive_cluster() {
    if !enabled() {
        eprintln!("skipping DuckLake e2e (set DEDUPE_E2E_DUCKLAKE=1 to run)");
        return;
    }

    /// Resolve base_10 under a config, with the model compiled from that config.
    /// Returns the partition and the scored pairs.
    fn resolve_compiled(
        tag: &str,
        yaml: &str,
    ) -> (BTreeSet<BTreeSet<String>>, BTreeSet<(String, String)>) {
        let config = load_config_str(yaml).expect("the config loads");
        let compiled = er_compile::compile(&config).expect("the config compiles");
        let tmp = std::env::temp_dir().join(format!("er_cli_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("data")).unwrap();
        let catalog = tmp.join("catalog.ducklake").to_string_lossy().into_owned();
        let data_path = tmp.join("data").to_string_lossy().into_owned();
        let session = er_cli::open_lake(&catalog, &data_path).unwrap();
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
                    std::fs::read_to_string(repo_fixture(&format!("base_10/base/{s}.csv")))
                        .unwrap(),
                )
            })
            .collect();
        let tf_csv = repo_fixture("model_test_v1.tf.csv");
        er_cli::run_all(
            &session,
            &config,
            &er_cli::PipelineInputs {
                mode: "full",
                rebuild_reason: None,
                segment: None,
                lexicon: &lexicon,
                deliveries: &deliveries,
                model: &er_cli::MatchModel {
                    settings_json: &compiled.model_json,
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
        let partition = partition(&membership_rows(&session));
        let scored: BTreeSet<(String, String)> = session
            .query_strings(&format!(
                "select rec_a_key, rec_b_key from {}",
                er_cli::MATCH_SCORES
            ))
            .unwrap()
            .into_iter()
            .map(|r| (r[0].clone(), r[1].clone()))
            .collect();
        drop(session);
        let _ = std::fs::remove_dir_all(&tmp);
        (partition, scored)
    }

    let pair = ("crm:C001".to_string(), "crm:C009".to_string());
    let base_yaml = include_str!("fixtures/test.yaml");
    let (unconstrained, scored_before) = resolve_compiled("veto_off", base_yaml);
    assert!(
        scored_before.contains(&pair),
        "precondition: without the veto this pair IS scored"
    );
    assert!(
        unconstrained
            .iter()
            .any(|c| c.contains("crm:C001") && c.contains("crm:C009")),
        "precondition: without the veto these two Chen records resolve together"
    );

    // Declare the veto. Inserted as YAML so the whole config path runs.
    let vetoed_yaml = base_yaml.replace(
        "comparisons:\n",
        "constraints:\n  veto: [email]\n\ncomparisons:\n",
    );
    assert_ne!(
        vetoed_yaml, base_yaml,
        "the constraints block must be added"
    );
    let config = load_config_str(&vetoed_yaml).expect("the constrained config loads");
    assert_eq!(
        config.constraints.veto,
        vec!["email".to_string()],
        "the veto must survive config loading"
    );

    let (vetoed, scored_after) = resolve_compiled("veto_on", &vetoed_yaml);

    // THE GUARANTEE: the conflicting pair is never scored.
    assert!(
        !scored_after.contains(&pair),
        "a veto on email must keep two records with DIFFERENT non-blank emails out of scoring \
         entirely, but the pair was scored"
    );
    assert!(
        scored_after.len() < scored_before.len(),
        "a veto can only remove candidates: {} scored with the veto vs {} without",
        scored_after.len(),
        scored_before.len()
    );
    // A record whose email normalizes to NULL is still eligible — a veto refuses
    // a conflict, it does not require a value.
    assert!(
        scored_after
            .iter()
            .any(|(a, b)| a == "webforms:W001" || b == "webforms:W001"),
        "the placeholder-email record must remain a candidate: a veto tolerates a missing value"
    );

    // THE LIMIT, asserted rather than assumed: the two are still co-clustered,
    // because each reaches the other through the NULL-email record.
    assert!(
        vetoed
            .iter()
            .any(|c| c.contains("crm:C001") && c.contains("crm:C009")),
        "candidate-level vetoes do not survive transitive closure; if this now passes, \
         entity-level enforcement has landed and this test should assert the stronger property"
    );

    // And a constraint changes how records group, never which records exist.
    let members = |p: &BTreeSet<BTreeSet<String>>| -> BTreeSet<String> {
        p.iter().flatten().cloned().collect()
    };
    assert_eq!(members(&vetoed), members(&unconstrained));
}

/// `required_match` is the stricter sibling: a pair is not a candidate unless
/// the named column is equal AND present on both sides. Declaring it on email
/// means only records with the same non-blank email can ever merge, which on
/// base_10 should collapse almost every cluster.
#[test]
fn a_required_match_restricts_candidates_to_pairs_that_agree_on_it() {
    if !enabled() {
        eprintln!("skipping DuckLake e2e (set DEDUPE_E2E_DUCKLAKE=1 to run)");
        return;
    }
    let base_yaml = include_str!("fixtures/test.yaml");
    let required_yaml = base_yaml.replace(
        "comparisons:\n",
        "constraints:\n  required_match: [email]\n\ncomparisons:\n",
    );
    assert_ne!(required_yaml, base_yaml);
    let config = load_config_str(&required_yaml).unwrap();
    let compiled = er_compile::compile(&config).unwrap();
    // The conjunct reached every blocking rule, including its presence half —
    // without that, two records both missing an email would "agree".
    let rules: Vec<String> = match serde_json::from_str::<serde_json::Value>(&compiled.model_json) {
        Ok(v) => v["blocking_rules_to_generate_predictions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["blocking_rule"].as_str().unwrap().to_string())
            .collect(),
        Err(e) => panic!("the compiled model must be JSON: {e}"),
    };
    assert_ne!(rules.len(), 0);
    for rule in &rules {
        assert!(
            rule.contains("l.email = r.email")
                && rule.contains("l.email IS NOT NULL")
                && rule.contains("r.email IS NOT NULL"),
            "every blocking rule must carry the required-match conjunct: {rule}"
        );
    }
}

/// A model compiled from the tenant config must RESOLVE, not merely parse.
///
/// The config's `comparisons:` / `blocking:` blocks were authored, validated at
/// publish, hashed into `config_hash` — and never read by anything that ran.
/// `er-compile` reads them, and its own tests prove the emitted document has the
/// committed model's structure. Structure is not the claim that matters, though:
/// what matters is whether the compiled model finds the same entities.
///
/// So this runs the whole pipeline twice over base_10 — once with the committed
/// EM-trained model, once with the model compiled out of the config that
/// describes it — and compares the partitions. Agreement means the DSL was an
/// accurate description of the matching all along, and that a tenant editing
/// `comparisons:` now gets the behaviour the config claims.
#[test]
fn a_model_compiled_from_the_config_resolves_base_10_like_the_trained_one() {
    if !enabled() {
        eprintln!("skipping DuckLake e2e (set DEDUPE_E2E_DUCKLAKE=1 to run)");
        return;
    }

    /// Resolve base_10 with the given settings document and return the partition.
    fn resolve_with(tag: &str, settings_json: &str) -> BTreeSet<BTreeSet<String>> {
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
                    std::fs::read_to_string(repo_fixture(&format!("base_10/base/{s}.csv")))
                        .unwrap(),
                )
            })
            .collect();
        let tf_csv = repo_fixture("model_test_v1.tf.csv");
        er_cli::run_all(
            &session,
            &config,
            &er_cli::PipelineInputs {
                mode: "full",
                rebuild_reason: None,
                segment: None,
                lexicon: &lexicon,
                deliveries: &deliveries,
                model: &er_cli::MatchModel {
                    settings_json,
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
        let partition = partition(&membership_rows(&session));
        drop(session);
        let _ = std::fs::remove_dir_all(&tmp);
        partition
    }

    let config = load_config_str(include_str!("fixtures/test.yaml")).unwrap();
    let compiled = er_compile::compile(&config).expect("the committed config compiles");

    let trained_json = std::fs::read_to_string(repo_fixture("model_test_v1.json")).unwrap();
    let trained = resolve_with("compile_trained", &trained_json);
    let from_config = resolve_with("compile_declared", &compiled.model_json);

    // The committed golden is the reference both must meet.
    let golden = membership_golden("base_10", "base");
    assert_eq!(
        trained, golden,
        "sanity: the trained model must still reproduce base_10's committed membership"
    );
    assert_eq!(
        from_config, golden,
        "a model compiled from the config must resolve base_10 into the same entities as the \
         trained model.\n  compiled: {from_config:#?}\n  golden: {golden:#?}"
    );
}

/// The run ledger's version columns, and the drift guard that reads them.
///
/// Three things were disconnected before this: `write_run_start` hardcoded
/// `mode = "full"` so an incremental run was indistinguishable in the ledger;
/// `lexicon_hash` and `rebuild_reason` were declared columns that nothing ever
/// wrote; and `er_versions::check_mode_preconditions` — the guard that decides
/// whether an incremental run may extend the existing corpus — had zero callers
/// anywhere in the workspace.
#[test]
fn the_run_ledger_records_the_fingerprint_the_drift_guard_reads() {
    if !enabled() {
        eprintln!("skipping DuckLake e2e (set DEDUPE_E2E_DUCKLAKE=1 to run)");
        return;
    }
    let (session, config, tmp) = run_all_base("ledger_fingerprint");

    // The full run wrote its real mode and a lexicon hash.
    let rows: Vec<(String, Option<String>)> = session
        .connection()
        .prepare("select mode, lexicon_hash from lake.main.runs order by started_at desc")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(rows.len(), 1, "one run so far");
    assert_eq!(rows[0].0, "full");
    let lexicon_hash = rows[0]
        .1
        .clone()
        .expect("the ledger must record the lexicon hash the run scored under");
    assert_ne!(lexicon_hash, "");

    // The run has not been marked succeeded, so there is no prior fingerprint to
    // guard against yet — and a first run is never guarded.
    let lexicon: Vec<(String, String)> = include_str!("fixtures/nickname_variants.csv")
        .lines()
        .skip(1)
        .filter(|l| !l.is_empty())
        .map(|l| {
            let (a, b) = l.split_once(',').unwrap();
            (a.to_string(), b.to_string())
        })
        .collect();
    let current = er_cli::current_fingerprint(&config, "v0001", &lexicon);
    assert_eq!(
        current.lexicon_hash.as_deref(),
        Some(lexicon_hash.as_str()),
        "the fingerprint the guard computes must equal the one the ledger stored"
    );

    er_cli::write_run_end(
        &session,
        "01RUNALLFULL0000000000000000",
        "2026-01-04 00:00:00",
    )
    .unwrap();
    let prior = er_cli::prior_fingerprint(&session)
        .unwrap()
        .expect("a succeeded run is a prior fingerprint");
    assert_eq!(prior, current, "nothing drifted between the two reads");

    // An unchanged fingerprint lets an incremental run proceed as incremental.
    let decision = er_versions::check_mode_preconditions(
        Some(&prior),
        &current,
        er_versions::MODE_INCREMENTAL,
        true,
    )
    .unwrap();
    assert_eq!(decision.mode, er_versions::MODE_INCREMENTAL);
    assert!(!decision.escalated(), "no drift -> no promotion");

    // A moved lexicon is drift: the same names no longer mean the same thing, so
    // the corpus cannot be extended incrementally. With escalation allowed, the
    // run is promoted to full instead of refused.
    let mut moved = lexicon.clone();
    moved.push(("zelda".to_string(), "zel".to_string()));
    let drifted = er_cli::current_fingerprint(&config, "v0001", &moved);
    assert_ne!(drifted.lexicon_hash, prior.lexicon_hash);
    let decision = er_versions::check_mode_preconditions(
        Some(&prior),
        &drifted,
        er_versions::MODE_INCREMENTAL,
        true,
    )
    .unwrap();
    assert!(decision.escalated(), "a moved lexicon must promote the run");
    assert_eq!(decision.mode, er_versions::MODE_FULL);
    assert!(
        decision.drift.iter().any(|d| d.field == "lexicon_hash"),
        "the drift must name the field that moved: {:?}",
        decision.drift
    );

    // Without escalation the same drift is a refusal, which is what the CLI's
    // --mode incremental does for an operator.
    let decision = er_versions::check_mode_preconditions(
        Some(&prior),
        &drifted,
        er_versions::MODE_INCREMENTAL,
        false,
    )
    .unwrap();
    assert!(decision.refused());

    let _ = std::fs::remove_dir_all(&tmp);
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
        &load_config_str(include_str!("fixtures/test.yaml")).unwrap(),
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

/// PR 1.3c through the lake: a protected record is never-cut, and the resulting
/// unsatisfiable `never` surfaces as an open `never_unsatisfiable` review. Three
/// records from distinct entities are chained A=B=C by two `always` edges; the
/// middle (B) is protected and a `never(A, C)` is added. The only A↔C path is the
/// always chain — uncuttable by probability AND blocked by B's protection — so the
/// fixpoint escalates instead of severing anything: A, B, C stay co-clustered and
/// a review is written. (The pure cut-vs-escalate logic is unit-tested in
/// er-reconcile; this exercises `load_protected_keys` + the review surfacing wired
/// through DuckLake.)
#[test]
fn protected_record_is_never_cut_and_escalates_through_ducklake() {
    if !enabled() {
        eprintln!("skipping DuckLake e2e (set DEDUPE_E2E_DUCKLAKE=1 to run)");
        return;
    }
    let (session, config, tmp) = standardized_lake("protect");
    match_model(&session);
    er_cli::run_reconcile(
        &session,
        &config,
        "01RUNPBASE0000000000000000",
        "2026-01-03 00:00:00",
    )
    .unwrap();

    // One record from each of three distinct entities (deterministic by sort).
    let base = membership_rows(&session);
    let mut by_entity: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (e, r) in &base {
        by_entity.entry(e.clone()).or_default().push(r.clone());
    }
    let mut ents: Vec<(String, Vec<String>)> = by_entity.into_iter().collect();
    ents.sort();
    assert!(ents.len() >= 3, "need three entities to chain");
    for e in &mut ents {
        e.1.sort();
    }
    let (ra, rb, rc) = (
        ents[0].1[0].clone(),
        ents[1].1[0].clone(),
        ents[2].1[0].clone(),
    );
    assert!(ra != rb && rb != rc && ra != rc, "three distinct records");

    // Chain A=B=C with two `always` edges (prob 1.0, model-independent).
    for (i, (x, y)) in [(&ra, &rb), (&rb, &rc)].iter().enumerate() {
        let always =
            serde_json::json!({ "type": "add_assertion", "kind": "always", "a": x, "b": y });
        er_cli::apply_staged_action(
            &session,
            &format!("act-always-{i}"),
            &always,
            "2026-01-04 00:00:00",
        )
        .unwrap();
    }
    // Protect the middle record and forbid the endpoints.
    let protect =
        serde_json::json!({ "type": "protect_record", "record_key": rb, "created_by": "steward" });
    er_cli::apply_staged_action(&session, "act-protect", &protect, "2026-01-04 12:00:00").unwrap();
    let never = serde_json::json!({ "type": "add_assertion", "kind": "never", "a": ra, "b": rc });
    er_cli::apply_staged_action(&session, "act-never", &never, "2026-01-05 00:00:00").unwrap();

    er_cli::run_reconcile(
        &session,
        &config,
        "01RUNPROT00000000000000000",
        "2026-01-06 00:00:00",
    )
    .unwrap();

    // Nothing was cut: A, B, C remain in one entity (B, protected, never severed).
    let after = membership_rows(&session);
    let entity_of = |rec: &str| -> Option<String> {
        after.iter().find(|(_, r)| r == rec).map(|(e, _)| e.clone())
    };
    let ea = entity_of(&ra).expect("A present after reconcile");
    assert_eq!(
        entity_of(&rb),
        Some(ea.clone()),
        "the protected middle record is never cut out"
    );
    assert_eq!(
        entity_of(&rc),
        Some(ea),
        "the endpoint stays co-clustered — the never escalated, nothing was cut"
    );

    // The unsatisfiable never surfaced as an open `never_unsatisfiable` review.
    let (ca, cb) = if ra <= rc {
        (ra.clone(), rc.clone())
    } else {
        (rc.clone(), ra.clone())
    };
    let reviews = session
        .query_strings(
            "select rec_a_key, rec_b_key, status from lake.main.review_queue \
             where reason = 'never_unsatisfiable'",
        )
        .unwrap();
    assert!(
        reviews
            .iter()
            .any(|r| r[0] == ca && r[1] == cb && r[2] == "open"),
        "expected an open never_unsatisfiable review for ({ca}, {cb}), got {reviews:?}"
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

/// The posterity gate, live half: after the pipeline has run every stage over a
/// real DuckLake catalog, `lake.main` contains ONLY relations the S5 registry
/// declares. The static half (`er-schema`'s `declared_names` test) catches
/// hardcoded names; this catches a dynamically-built one — however a relation
/// gets created, an undeclared survivor in the lake fails here.
#[test]
fn the_lake_contains_only_declared_relations() {
    if !enabled() {
        eprintln!("skipping lake declared-relations gate (set DEDUPE_E2E_DUCKLAKE=1)");
        return;
    }
    let (session, config, tmp) = standardized_lake("declared_only");
    match_model(&session);
    er_cli::run_reconcile(
        &session,
        &config,
        "01RUNDECLAREDONLY000000000",
        "2026-01-02 00:00:00",
    )
    .unwrap();
    er_cli::run_golden(&session, &config, "2026-01-03 00:00:00").unwrap();

    let declared: BTreeSet<String> = er_schema::model::all_specs()
        .into_iter()
        .map(|s| s.name)
        .collect();
    let live = session
        .query_strings(
            "select table_name from information_schema.tables \
             where table_catalog = 'lake' and table_schema = 'main'",
        )
        .unwrap();
    let undeclared: Vec<String> = live
        .into_iter()
        .map(|row| row[0].clone())
        .filter(|name| !declared.contains(name))
        .collect();
    assert!(
        undeclared.is_empty(),
        "lake.main relations that no schema authority declares (add the relation \
         to er_schema::model or the entity catalog before creating it): {undeclared:?}"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

/// Activating a new model version must retire the model it replaces — and the
/// consequence that matters is that a STRICTER model can actually tighten the
/// result.
///
/// Scores are keyed by `(pair, model_version, tf_snapshot_id)`, so a run under a
/// new model writes NEW rows and left the previous model's rows `is_active`.
/// Neither edge read filters by model — `auto_merge_edges_live`, which reconcile
/// clusters from, and `active_page`, which the duplicates surface reads, both
/// take every active row. So two models' verdicts were unioned, silently and for
/// good: 19 pairs became 38 active scores. The sharp end is this test's second
/// half — a model built to be stricter could not change anything, because the
/// looser model it replaced went on merging the records.
#[test]
fn activating_a_new_model_retires_the_one_it_replaces() {
    if !enabled() {
        eprintln!("skipping DuckLake e2e (set DEDUPE_E2E_DUCKLAKE=1 to run)");
        return;
    }
    let (session, config, tmp) = open_seeded_lake("model_retire");
    run_all_phase(
        &session,
        &config,
        "incremental_batch",
        "base",
        "01MODRET00000000000BASE000",
        "2026-01-01 00:00:00",
        "full",
    );
    let loose = active_scores(&session);
    let loose_partition = partition(&membership_rows(&session));
    assert!(!loose.is_empty(), "precondition: the first model scored");

    // A second model identity whose settings are deliberately STRICTER: every
    // comparison level is kept, but the match weights are scaled down so fewer
    // pairs clear `review_low`. Same corpus, same TF snapshot id is NOT reused —
    // identity is (model_version, tf_snapshot_id) and both move.
    let model_json = std::fs::read_to_string(repo_fixture("model_test_v1.json")).unwrap();
    let mut settings: serde_json::Value = serde_json::from_str(&model_json).unwrap();
    // Drop the prior hard, which shifts every posterior down.
    settings["probability_two_random_records_match"] = serde_json::json!(1.0e-9);
    let strict_json = settings.to_string();
    let tf_csv = repo_fixture("model_test_v1.tf.csv");

    er_cli::run_match(
        &session,
        &config,
        &er_cli::MatchModel {
            settings_json: &strict_json,
            tf_csv_path: std::path::Path::new(&tf_csv),
            model_version: "v0002-strict",
            tf_snapshot_id: "01JWMDTFSNAP00000000000002",
        },
        config.thresholds.review_low,
        "01MODRET0000000000STRICT00",
        "2026-02-01 00:00:00",
    )
    .unwrap();

    // Exactly one model identity may be active.
    let by_model = session
        .query_strings(&format!(
            "select model_version, cast(count(*) as varchar)
             from {} where is_active group by model_version order by model_version",
            er_cli::MATCH_SCORES
        ))
        .unwrap();
    assert_eq!(
        by_model.len(),
        1,
        "only the live model's scores may be active, found {by_model:?}"
    );
    assert_eq!(by_model[0][0], "v0002-strict");

    // The retired rows are still THERE — retiring is not deleting, so the
    // previous model's verdicts stay auditable and a review can still cite them.
    let retired = session
        .query_strings(&format!(
            "select cast(count(*) as varchar) from {} \
             where not is_active and model_version = 'v0001'",
            er_cli::MATCH_SCORES
        ))
        .unwrap();
    assert_eq!(
        retired[0][0],
        loose.len().to_string(),
        "every one of the old model's scores should be retired, not dropped"
    );
    let stamped = session
        .query_strings(&format!(
            "select cast(count(*) as varchar) from {} \
             where not is_active and invalidated_run_id = '01MODRET0000000000STRICT00' \
               and invalidated_at is not null",
            er_cli::MATCH_SCORES
        ))
        .unwrap();
    assert_eq!(
        stamped[0][0],
        loose.len().to_string(),
        "retirement must record which run did it and when"
    );

    // And the point of all of it: the stricter model now decides the clustering.
    let strict = active_scores(&session);
    assert!(
        strict.len() < loose.len(),
        "precondition: the second model must really be stricter ({} vs {} pairs)",
        strict.len(),
        loose.len()
    );
    er_cli::run_reconcile(
        &session,
        &config,
        "01MODRET000000000000RECON0",
        "2026-02-01 00:00:00",
    )
    .unwrap();
    let strict_partition = partition(&membership_rows(&session));
    assert!(
        strict_partition.len() > loose_partition.len(),
        "a stricter model must split entities the looser one merged; \
         {} entities before, {} after",
        loose_partition.len(),
        strict_partition.len()
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

/// The labelling queue must offer the pairs the model is UNSURE about.
///
/// This is uncertainty sampling, and it is the difference between a labelling
/// session that teaches the model something and one that wastes a human's
/// attention. A queue ordered by score descending offers the pairs it is
/// already most confident about — which confirm what it knows and move its m/u
/// estimates almost not at all. The pairs either side of the decision threshold
/// are where it is actually wrong.
///
/// Asserted here rather than in the Postgres tier because it needs real scores
/// to order: the ordering is a property of `match_scores`, and a test over an
/// empty or synthetic table could not tell the two orderings apart.
#[test]
fn the_labeling_queue_offers_the_least_certain_pairs_first() {
    if !enabled() {
        eprintln!("skipping DuckLake e2e (set DEDUPE_E2E_DUCKLAKE=1 to run)");
        return;
    }
    let (session, config, tmp) = run_all_base("labeling_queue");
    let threshold = config.thresholds.auto_merge;

    let page = er_entities::match_score::read::uncertain_page(
        session.connection(),
        session.latest_snapshot_version().unwrap(),
        threshold,
        config.thresholds.review_low,
        1.0,
        50,
    )
    .unwrap();
    assert!(
        page.len() >= 5,
        "base_10 should offer several scored pairs to label, got {}",
        page.len()
    );

    // Non-decreasing distance from the threshold — the ordering claim itself.
    let distance = |p: f64| (p - threshold).abs();
    let mut previous = f64::NEG_INFINITY;
    for row in &page {
        let d = distance(row.match_probability);
        assert!(
            d >= previous - 1e-12,
            "pairs must arrive least-certain first, but the distance from the \
             threshold went {previous} then {d}"
        );
        previous = d;
    }

    // And the ordering is not accidentally the same as by-probability, or the
    // assertion above would hold for a queue that does the useless thing.
    let by_probability: Vec<String> = {
        let mut rows: Vec<_> = page.iter().collect();
        rows.sort_by(|a, b| b.match_probability.total_cmp(&a.match_probability));
        rows.iter()
            .map(|r| format!("{}|{}", r.rec_a_key, r.rec_b_key))
            .collect()
    };
    let as_served: Vec<String> = page
        .iter()
        .map(|r| format!("{}|{}", r.rec_a_key, r.rec_b_key))
        .collect();
    assert_ne!(
        as_served, by_probability,
        "the uncertainty order must differ from score order on this corpus, or \
         this test cannot distinguish the two"
    );

    // The first pair offered really is the closest to the threshold in the
    // whole active set, not merely first in whatever order came back.
    let closest = page
        .iter()
        .map(|r| distance(r.match_probability))
        .fold(f64::INFINITY, f64::min);
    assert!(
        (distance(page[0].match_probability) - closest).abs() < 1e-12,
        "the first pair offered is not the least certain one"
    );

    // Deterministic: the same request returns the same page, so a labeller who
    // reloads does not get a reshuffled list.
    let again = er_entities::match_score::read::uncertain_page(
        session.connection(),
        session.latest_snapshot_version().unwrap(),
        threshold,
        config.thresholds.review_low,
        1.0,
        50,
    )
    .unwrap();
    assert_eq!(
        again
            .iter()
            .map(|r| format!("{}|{}", r.rec_a_key, r.rec_b_key))
            .collect::<Vec<_>>(),
        as_served,
        "the queue must be deterministic across identical requests"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

/// A run must leave its metrics behind, and they must agree with the corpus.
///
/// Every dashboard number has always been computed live from the lake, so the
/// product could say what is true now and nothing about the direction of
/// travel — and "is the duplicate rate going up" is the question an operator
/// actually has. Nothing was retained to answer it.
///
/// The counts are checked against the lake they describe rather than against
/// constants, so the test says "the snapshot is true" instead of "the snapshot
/// is what it was when I wrote this".
#[test]
fn a_run_records_metrics_that_agree_with_the_corpus() {
    if !enabled() {
        eprintln!("skipping DuckLake e2e (set DEDUPE_E2E_DUCKLAKE=1 to run)");
        return;
    }
    let (session, config, tmp) = run_all_base("metrics");

    let snapshots = er_entities::metric_snapshot::read::recent(
        session.connection(),
        session.latest_snapshot_version().unwrap(),
        10,
    )
    .unwrap();
    assert_eq!(
        snapshots.len(),
        1,
        "a run should leave exactly one snapshot, got {}",
        snapshots.len()
    );
    let snap = &snapshots[0];
    assert_eq!(snap.run_id, "01RUNALLFULL0000000000000000");

    // Against the corpus it describes, not against constants.
    let one = |sql: &str| -> i64 {
        session.query_strings(sql).unwrap()[0][0]
            .parse()
            .unwrap_or(-1)
    };
    let records = one(&format!("select count(*) from {}", er_cli::INT_STD_RECORDS));
    let partition = partition(&membership_rows(&session));
    let entities = i64::try_from(partition.len()).unwrap();
    let dup_groups = i64::try_from(partition.iter().filter(|c| c.len() > 1).count()).unwrap();
    let dup_records: i64 = partition
        .iter()
        .filter(|c| c.len() > 1)
        .map(|c| i64::try_from(c.len()).unwrap())
        .sum();
    let largest = partition
        .iter()
        .map(|c| i64::try_from(c.len()).unwrap())
        .max()
        .unwrap_or(0);

    assert_eq!(snap.records, records, "records");
    assert_eq!(snap.entities, entities, "entities");
    assert_eq!(snap.duplicate_groups, dup_groups, "duplicate groups");
    assert_eq!(
        snap.duplicate_records, dup_records,
        "duplicate RECORDS, not groups — counting groups calls a five-record \
         cluster and a two-record cluster the same amount of duplication"
    );
    assert_eq!(snap.largest_group, largest, "largest group");
    assert!(
        snap.duplicate_records > snap.duplicate_groups,
        "base_10 has clusters bigger than two, so these must differ — if they \
         are equal the test cannot tell the two definitions apart"
    );

    // The threshold bands, against the scores they summarise.
    let auto = one(&format!(
        "select count(*) from {} where is_active and match_probability >= {}",
        er_cli::MATCH_SCORES,
        config.thresholds.auto_merge
    ));
    assert_eq!(snap.auto_merge_pairs, auto, "auto-merge pairs");
    assert!(
        snap.auto_merge_pairs > 0,
        "base_10 should have merged something"
    );

    // Re-running the same id CORRECTS the snapshot instead of adding a second —
    // a correction pass and a retry both reuse a run id.
    er_cli::write_metric_snapshot(
        &session,
        &config,
        "01RUNALLFULL0000000000000000",
        "2026-02-01 00:00:00",
    )
    .unwrap();
    let after = er_entities::metric_snapshot::read::recent(
        session.connection(),
        session.latest_snapshot_version().unwrap(),
        10,
    )
    .unwrap();
    assert_eq!(after.len(), 1, "re-running a run id must not double-count");
    assert!(
        after[0].captured_at.to_string().starts_with("2026-02-01"),
        "the correction should have moved the captured_at, got {}",
        after[0].captured_at
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

/// Per-record quality scoring, and the judgement that makes it mean something.
///
/// The easy version of this feature scores a missing email as both incomplete
/// AND invalid, which penalises one absence twice and makes "fill in the
/// blanks" look like it improves validity. So the assertions below are mostly
/// about the dimensions being INDEPENDENT: a record missing a field must lose
/// completeness without losing validity, and a record whose source publishes no
/// timestamp must not be dragged to the bottom of the work queue for it.
#[test]
fn record_quality_scores_each_dimension_only_where_it_applies() {
    if !enabled() {
        eprintln!("skipping DuckLake e2e (set DEDUPE_E2E_DUCKLAKE=1 to run)");
        return;
    }
    let (session, base_config, tmp) = run_all_base("quality");

    // Unconfigured writes nothing at all — the table's absence is the no-op.
    assert!(
        !er_cli::write_record_quality(
            &session,
            &base_config,
            "01QUALITY0000000000000000000",
            "2026-01-03 00:00:00"
        )
        .unwrap(),
        "an unset data_quality block must score nothing"
    );

    let config = with_data_quality(&base_config);
    assert!(
        er_cli::write_record_quality(
            &session,
            &config,
            "01QUALITY0000000000000000000",
            "2026-01-03 00:00:00"
        )
        .unwrap(),
        "a configured block must score"
    );

    let rows = er_entities::record_quality::read::worst(
        session.connection(),
        session.latest_snapshot_version().unwrap(),
        100.0,
        500,
    )
    .unwrap();
    let records: i64 = session
        .query_strings(&format!("select count(*) from {}", er_cli::INT_STD_RECORDS))
        .unwrap()[0][0]
        .parse()
        .unwrap();
    assert_eq!(
        i64::try_from(rows.len()).unwrap(),
        records,
        "every record in the corpus should be scored"
    );

    for r in &rows {
        assert!(
            r.score.is_finite() && (0.0..=100.0).contains(&r.score),
            "{} scored {}",
            r.record_key,
            r.score
        );
        assert!((0.0..=1.0).contains(&r.completeness));
        if let Some(v) = r.validity {
            assert!((0.0..=1.0).contains(&v), "validity {v}");
        }
        if let Some(f) = r.freshness {
            assert!((0.0..=1.0).contains(&f), "freshness {f}");
        }
    }

    // The scores must DISCRIMINATE. base_10 deliberately contains records with
    // missing emails and missing phones, so a scorer that works will spread
    // them out; one that returns a constant would pass every assertion above.
    let min = rows.iter().map(|r| r.score).fold(f64::INFINITY, f64::min);
    let max = rows
        .iter()
        .map(|r| r.score)
        .fold(f64::NEG_INFINITY, f64::max);
    assert!(
        max - min > 1.0,
        "the scores must distinguish records (min {min}, max {max}) — a constant \
         score would satisfy every range check above"
    );

    // The judgement: a record MISSING its email loses completeness and is NOT
    // marked invalid for it. base_10's crm:C008 has no email address.
    let incomplete = rows
        .iter()
        .find(|r| r.record_key == "crm:C008")
        .expect("crm:C008 is in base_10");
    let complete = rows
        .iter()
        .find(|r| r.record_key == "crm:C001")
        .expect("crm:C001 is in base_10");
    assert!(
        incomplete.completeness < complete.completeness,
        "a record with no email must be less complete ({} vs {})",
        incomplete.completeness,
        complete.completeness
    );
    assert_eq!(
        incomplete.validity.map(|v| v >= 1.0),
        Some(true),
        "a missing email must not count as an INVALID email — that penalises \
         one absence twice; crm:C008's remaining validatable fields are fine, \
         so its validity is 1.0, got {:?}",
        incomplete.validity
    );

    // Re-scoring corrects rather than duplicating.
    er_cli::write_record_quality(&session, &config, "01QUALITY2", "2026-06-01 00:00:00").unwrap();
    let again = er_entities::record_quality::read::worst(
        session.connection(),
        session.latest_snapshot_version().unwrap(),
        100.0,
        500,
    )
    .unwrap();
    assert_eq!(
        again.len(),
        rows.len(),
        "re-scoring must not duplicate rows"
    );
    // Five months later, the same records are staler, so scores must have moved
    // DOWN — freshness is actually being computed rather than stubbed.
    let before: f64 = rows.iter().map(|r| r.score).sum();
    let after: f64 = again.iter().map(|r| r.score).sum();
    assert!(
        after < before,
        "scoring the same corpus five months later must lower freshness \
         (total {after} vs {before})"
    );

    // The distribution read reports buckets, not just a mean — a mean of 70 can
    // be every record at 70 or half at 95 and half at 45.
    let dist = er_entities::record_quality::read::distribution(
        session.connection(),
        session.latest_snapshot_version().unwrap(),
    )
    .unwrap();
    let d = &dist[0];
    assert_eq!(d.scored, Some(i64::try_from(rows.len()).unwrap()));
    let bucketed =
        d.below_50.unwrap_or(0) + d.between_50_80.unwrap_or(0) + d.at_least_80.unwrap_or(0);
    assert_eq!(
        bucketed,
        i64::try_from(again.len()).unwrap(),
        "every scored record must land in exactly one bucket"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

/// A `data_quality` config, added in code so the committed fixture's
/// `config_hash` and the config golden stay untouched.
fn with_data_quality(config: &Config) -> Config {
    let mut c = config.clone();
    c.data_quality = er_config::DataQuality {
        completeness: [
            ("given_name".to_string(), 1.0),
            ("family_name".to_string(), 1.0),
            ("email".to_string(), 2.0),
            ("phone_e164".to_string(), 1.0),
            ("birth_date".to_string(), 1.0),
        ]
        .into_iter()
        .collect(),
        weights: er_config::DataQualityWeights {
            completeness: 1.0,
            validity: 1.0,
            freshness: 1.0,
        },
        fresh_days: 90,
        stale_days: 1095,
    };
    c
}

/// Cleanse rules must change the standardized corpus — and the absence of them
/// must change nothing at all.
///
/// Both halves matter. The second is the determinism contract: every
/// `std_hashes` golden in the repo was produced without a `transforms` block,
/// so a config with none must produce byte-identical staging SQL. The first is
/// the feature: a rule that compiles and has no effect is the failure mode a
/// config DSL falls into (this repo has already had one — the `comparisons`
/// levels nothing compiled, #216).
#[test]
fn cleanse_rules_change_the_corpus_and_their_absence_changes_nothing() {
    if !enabled() {
        eprintln!("skipping DuckLake e2e (set DEDUPE_E2E_DUCKLAKE=1 to run)");
        return;
    }
    use er_config::TransformRule;

    let (session, config, tmp) = standardized_lake("transform_none");
    let baseline: Vec<Vec<String>> = session
        .query_strings(&format!(
            "select record_key, coalesce(given_name,''), coalesce(email,'') \
             from {} order by record_key",
            er_cli::INT_STD_RECORDS
        ))
        .unwrap();
    assert!(!baseline.is_empty(), "the base corpus standardized");

    // Two rules, chosen to show the interaction with the normalizers rather
    // than hide it.
    //
    // A CASE rule on `given_name` is invisible in the stored value, because
    // cleanse runs BEFORE `name_norm` and `name_norm` lowercases. That is not a
    // bug and it is worth asserting: an operator reaching for "upper-case the
    // names" will get no visible change, and the reason is the ordering the
    // feature is built on.
    let mut cased = config.clone();
    cased.transforms = [(
        "given_name".to_string(),
        vec![TransformRule::Upper, TransformRule::Trim],
    )]
    .into_iter()
    .collect();
    er_cli::run_standardize(&session, &cased, "2026-01-01 00:00:00").unwrap();
    let after_case: Vec<Vec<String>> = session
        .query_strings(&format!(
            "select record_key, coalesce(given_name,''), coalesce(email,'') \
             from {} order by record_key",
            er_cli::INT_STD_RECORDS
        ))
        .unwrap();
    assert_eq!(
        after_case, baseline,
        "a case rule on a field the normalizer lowercases has no visible effect \
         — cleanse runs BEFORE standardization, and name_norm has the last word"
    );

    // A rule that changes the CHARACTERS does survive. base_10's
    // `webforms:W001` is "Bob"; the lexicon already treats Bob and Robert as
    // variants, so correcting it at source is a real cleanse operation.
    let mut ruled = config.clone();
    ruled.transforms = [(
        "given_name".to_string(),
        vec![TransformRule::Dictionary {
            map: [("Bob".to_string(), "Robert".to_string())]
                .into_iter()
                .collect(),
        }],
    )]
    .into_iter()
    .collect();
    er_cli::run_standardize(&session, &ruled, "2026-01-01 00:00:00").unwrap();
    let after: Vec<Vec<String>> = session
        .query_strings(&format!(
            "select record_key, coalesce(given_name,''), coalesce(email,'') \
             from {} order by record_key",
            er_cli::INT_STD_RECORDS
        ))
        .unwrap();
    assert_eq!(after.len(), baseline.len(), "same records");
    assert_ne!(
        after, baseline,
        "a dictionary rule must change the corpus — a rule that compiles and \
         does nothing is exactly the trap the uncompiled comparisons DSL fell \
         into"
    );

    // Specifically the targeted record and column, and only those.
    let changed: Vec<&Vec<String>> = after
        .iter()
        .zip(&baseline)
        .filter(|(a, b)| a[1] != b[1])
        .map(|(a, _)| a)
        .collect();
    assert_eq!(
        changed.len(),
        1,
        "exactly the one record whose given name was Bob: {changed:?}"
    );
    assert_eq!(changed[0][0], "webforms:W001");
    assert_eq!(
        changed[0][1], "robert",
        "the rule rewrote the raw value and the normalizer then lowercased it, \
         which is the ordering this feature is built on"
    );
    let changed_emails = after
        .iter()
        .zip(&baseline)
        .filter(|(a, b)| a[2] != b[2])
        .count();
    assert_eq!(
        changed_emails, 0,
        "a rule on given_name must not touch email"
    );

    // Re-standardizing with NO rules restores the baseline exactly — the
    // determinism contract every std_hashes golden depends on.
    er_cli::run_standardize(&session, &config, "2026-01-01 00:00:00").unwrap();
    let restored: Vec<Vec<String>> = session
        .query_strings(&format!(
            "select record_key, coalesce(given_name,''), coalesce(email,'') \
             from {} order by record_key",
            er_cli::INT_STD_RECORDS
        ))
        .unwrap();
    assert_eq!(
        restored, baseline,
        "a config with no transforms must standardize byte-identically"
    );

    // The preview shows only rows the rules CHANGE, against the RAW values —
    // previewing against standardized values would answer the wrong question.
    let preview =
        er_cli::preview_transform(&session, &config, "given_name", &[TransformRule::Upper], 5)
            .unwrap();
    assert!(
        !preview.is_empty(),
        "upper-casing mixed-case names should preview some changes"
    );
    for (key, before, after_val) in &preview {
        assert_ne!(
            before, after_val,
            "{key}: the preview must only list rows the rule changes"
        );
        assert_eq!(after_val, &before.to_uppercase());
    }

    // A dictionary passes unmatched values THROUGH rather than nulling them —
    // it is a correction list, not an allowlist, and the other behaviour would
    // silently delete most of a column.
    let dict = er_cli::preview_transform(
        &session,
        &config,
        "given_name",
        &[TransformRule::Dictionary {
            map: [("Bob".to_string(), "Robert".to_string())]
                .into_iter()
                .collect(),
        }],
        50,
    )
    .unwrap();
    for (key, before, after_val) in &dict {
        assert_eq!(
            (before.as_str(), after_val.as_str()),
            ("Bob", "Robert"),
            "{key}: only the mapped value should change"
        );
    }

    let _ = std::fs::remove_dir_all(&tmp);
}
