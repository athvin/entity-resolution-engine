//! The read-API entity-switch for Account orgs (behind `DEDUPE_E2E_DUCKLAKE=1`).
//! Builds a tiny Account lake with `er_cli::run_all` (the same corpus/model as
//! the er-cli `acct_e2e` gate), then exercises `read::golden_arrow_page` (the Arrow
//! list), `read::entity_detail` and `read::duplicates` through the Account entity —
//! proving the read layer serves the `acct_golden_records`/`acct_golden_lineage`
//! projection + the company/domain search when an org is `entity_type = account`.
//! The person projection is the default and is covered by `pg_golden_read`.

use std::path::{Path, PathBuf};

use arrow::array::{Array, StringArray};
use arrow::record_batch::RecordBatch;
use er_config::{EntityType, load_config_str};
use er_server::read::{self, GoldenArrowPage};

fn enabled() -> bool {
    std::env::var("DEDUPE_E2E_DUCKLAKE").as_deref() == Ok("1")
}

/// Total rows across an Arrow page's batches.
fn total_rows(batches: &[RecordBatch]) -> usize {
    batches.iter().map(RecordBatch::num_rows).sum()
}

/// Whether the page's schema declares a column by name.
fn has_field(page: &GoldenArrowPage, name: &str) -> bool {
    page.schema.fields().iter().any(|f| f.name() == name)
}

/// The non-null string values of a Utf8 column across a page's batches, in order.
fn string_values(batches: &[RecordBatch], col: &str) -> Vec<String> {
    let mut out = Vec::new();
    for batch in batches {
        let Ok(idx) = batch.schema().index_of(col) else {
            continue;
        };
        if let Some(arr) = batch.column(idx).as_any().downcast_ref::<StringArray>() {
            for i in 0..arr.len() {
                if arr.is_valid(i) {
                    out.push(arr.value(i).to_string());
                }
            }
        }
    }
    out
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

    let snapshot = read::latest_snapshot(&session).unwrap();

    // List: the 4 account entities, served as an Arrow page whose schema is the
    // acct_golden_records projection — it carries company_name and NOT a person
    // given_name, proving the read selected acct_golden_records.
    let page =
        read::golden_arrow_page(&session, EntityType::Account, snapshot, None, None, 50).unwrap();
    assert_eq!(
        total_rows(&page.batches),
        4,
        "4 account entities from the read API"
    );
    assert!(
        has_field(&page, "company_name"),
        "account projection carries company_name"
    );
    assert!(
        !has_field(&page, "given_name"),
        "account rows carry no person given_name column"
    );
    assert_eq!(
        string_values(&page.batches, "company_name").len(),
        4,
        "every account row has a company_name"
    );

    // Company/domain contains-search finds Acme (the person search over
    // given_name/family_name/email would never match a company name).
    let hits = read::golden_arrow_page(
        &session,
        EntityType::Account,
        snapshot,
        Some("acme"),
        None,
        50,
    )
    .unwrap();
    assert_eq!(
        total_rows(&hits.batches),
        1,
        "company_name search matches exactly Acme"
    );
    assert!(
        string_values(&hits.batches, "company_name")
            .iter()
            .any(|c| c.to_lowercase().contains("acme"))
    );

    // Detail: resolve one entity_id from the Arrow page, assert the Account golden
    // + lineage (from acct_golden_lineage) come back.
    let entity_id = string_values(&page.batches, "id")
        .into_iter()
        .next()
        .expect("an entity_id in the account page");
    let detail = read::entity_detail(&session, EntityType::Account, snapshot, &entity_id)
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
    let (groups, _) = read::duplicates(&session, EntityType::Account, snapshot, None, 50).unwrap();
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
