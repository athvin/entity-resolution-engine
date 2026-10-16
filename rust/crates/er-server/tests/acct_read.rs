//! The read-API entity-switch for Account orgs (behind `DEDUPE_E2E_DUCKLAKE=1`).
//! Builds a tiny Account lake with `er_cli::run_all` (the same corpus/model as
//! the er-cli `acct_e2e` gate), then exercises `read::golden_list` /
//! `read::entity_detail` through `GoldenView::account()` — proving the read layer
//! serves the `acct_golden_records`/`acct_golden_lineage` projection + the
//! company/domain search when an org is `entity_type = account`. The person
//! `GoldenView` is the default and is covered by the existing person read tests.

use std::path::{Path, PathBuf};

use er_config::load_config_str;
use er_server::read::{self, GoldenView};

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

#[test]
fn account_golden_read_serves_acct_projection() {
    if !enabled() {
        eprintln!("skipping Account read e2e (set DEDUPE_E2E_DUCKLAKE=1 to run)");
        return;
    }
    let tmp = std::env::temp_dir().join(format!("er_acct_read_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(tmp.join("data")).unwrap();
    let catalog = tmp.join("catalog.ducklake").to_string_lossy().into_owned();
    let data_path = tmp.join("data").to_string_lossy().into_owned();
    let session = er_cli::open_lake(&catalog, &data_path).unwrap();
    let config =
        load_config_str(&std::fs::read_to_string(repo_fixture("acct_base/config.yaml")).unwrap())
            .unwrap();
    let deliveries: Vec<(String, String)> = ["crm", "vendors"]
        .iter()
        .map(|s| {
            (
                (*s).to_string(),
                std::fs::read_to_string(repo_fixture(&format!("acct_base/{s}.csv"))).unwrap(),
            )
        })
        .collect();
    let model_json = std::fs::read_to_string(repo_fixture("acct_model_v1.json")).unwrap();
    let tf_csv = repo_fixture("acct_model_v1.tf.csv");
    er_cli::run_all(
        &session,
        &config,
        &er_cli::PipelineInputs {
            lexicon: &Vec::new(),
            deliveries: &deliveries,
            model: &er_cli::MatchModel {
                settings_json: &model_json,
                tf_csv_path: Path::new(&tf_csv),
                model_version: "acct_v1",
                tf_snapshot_id: "ACCTTFSNAP0000000000000001",
            },
            run_id: "01ACCTREAD00000000000000000",
            ingested_at: "2026-03-01 00:00:00",
            scored_at: "2026-03-02 00:00:00",
            run_at: "2026-03-03 00:00:00",
        },
    )
    .unwrap();

    let view = GoldenView::account();
    let snapshot = read::latest_snapshot(&session).unwrap();

    // List: the 4 account entities, each carrying a company_name (NOT a person
    // given_name), proving the read selected acct_golden_records.
    let (items, _next) = read::golden_list(&session, &view, snapshot, None, None, 50).unwrap();
    assert_eq!(items.len(), 4, "4 account entities from the read API");
    assert!(
        items
            .iter()
            .all(|it| it.get("company_name").and_then(|v| v.as_str()).is_some()),
        "every account row has a company_name"
    );
    assert!(
        items.iter().all(|it| it.get("given_name").is_none()),
        "account rows carry no person given_name column"
    );

    // Company/domain contains-search finds Acme (the person search over
    // given_name/family_name/email would never match a company name).
    let (hits, _) = read::golden_list(&session, &view, snapshot, Some("acme"), None, 50).unwrap();
    assert_eq!(hits.len(), 1, "company_name search matches exactly Acme");
    assert!(
        hits[0]
            .get("company_name")
            .and_then(|v| v.as_str())
            .is_some_and(|c| c.to_lowercase().contains("acme"))
    );

    // Detail: resolve one entity_id from the list, assert the Account golden +
    // lineage (from acct_golden_lineage) come back.
    let entity_id = items[0]
        .get("entity_id")
        .and_then(|v| v.as_str())
        .unwrap()
        .to_string();
    let detail = read::entity_detail(&session, &view, snapshot, &entity_id)
        .unwrap()
        .expect("entity detail present");
    assert!(
        detail["golden"].get("company_name").is_some(),
        "entity detail golden is the account projection"
    );
    assert!(
        detail["lineage"].is_array(),
        "account lineage (acct_golden_lineage) returned"
    );

    // Duplicates: the auto-merged groups the /duplicates endpoint serves, carrying
    // the ACCOUNT preview (company_name/domain) rather than person name/email.
    // Acme, Globex and Initech each pair a crm row with a vendors row; Umbrella
    // has no vendors twin and so is single-member, not a group.
    let (groups, _) = read::duplicates(&session, &view, snapshot, None, 50).unwrap();
    assert_eq!(
        groups.len(),
        3,
        "three cross-source account duplicate groups"
    );
    assert!(
        groups
            .iter()
            .all(|g| g.get("member_count").and_then(serde_json::Value::as_i64) == Some(2)),
        "each account group pairs the crm and vendors row"
    );
    assert!(
        groups
            .iter()
            .all(|g| g.get("company_name").and_then(|v| v.as_str()).is_some()),
        "each group carries the account company_name preview"
    );
    assert!(
        groups.iter().all(|g| g.get("domain").is_some()),
        "each group carries the account domain preview"
    );
    assert!(
        groups.iter().all(|g| g.get("given_name").is_none()),
        "account duplicate groups carry no person given_name preview"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}
