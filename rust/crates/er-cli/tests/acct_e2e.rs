//! Account entity-type end-to-end gate (behind `DEDUPE_E2E_DUCKLAKE=1`, like the
//! person `lake_e2e`): `run_all` over a tiny Account corpus with the committed
//! hand-authored `acct_model_v1`, asserting the Account matching + golden
//! assembly produce the expected entities. The person pipeline is untouched
//! (separate fixtures/model, and every pipeline branch is gated on
//! `entity_type == account`). Correctness is asserted behaviourally: records that
//! are the same company co-cluster, distinct companies do not, and
//! `acct_golden_records` assembles one row per entity.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use er_config::load_config_str;

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
fn acct_run_all_clusters_accounts_and_assembles_golden() {
    if !enabled() {
        eprintln!("skipping Account DuckLake e2e (set DEDUPE_E2E_DUCKLAKE=1 to run)");
        return;
    }
    let tmp = std::env::temp_dir().join(format!("er_cli_acct_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(tmp.join("data")).unwrap();
    let catalog = tmp.join("catalog.ducklake").to_string_lossy().into_owned();
    let data_path = tmp.join("data").to_string_lossy().into_owned();
    let session = er_cli::open_lake(&catalog, &data_path).unwrap();
    let config =
        load_config_str(&std::fs::read_to_string(repo_fixture("acct_base/config.yaml")).unwrap())
            .unwrap();
    assert!(
        !config.entity_type.is_person(),
        "fixture is an account config"
    );

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
    let lexicon: Vec<(String, String)> = Vec::new(); // accounts use no nickname lexicon

    er_cli::run_all(
        &session,
        &config,
        &er_cli::PipelineInputs {
            lexicon: &lexicon,
            deliveries: &deliveries,
            model: &er_cli::MatchModel {
                settings_json: &model_json,
                tf_csv_path: Path::new(&tf_csv),
                model_version: "acct_v1",
                tf_snapshot_id: "ACCTTFSNAP0000000000000001",
            },
            run_id: "01ACCTRUNALL0000000000000000",
            ingested_at: "2026-03-01 00:00:00",
            scored_at: "2026-03-02 00:00:00",
            run_at: "2026-03-03 00:00:00",
        },
    )
    .unwrap();

    // Membership: 7 account records → 4 entities (3 merged dup pairs + 1 singleton),
    // written to the SHARED entity-agnostic tables.
    let rows: Vec<(String, String)> = session
        .query_strings(
            "select record_key, entity_id from lake.main.entity_membership order by record_key",
        )
        .unwrap()
        .into_iter()
        .map(|r| (r[0].clone(), r[1].clone()))
        .collect();
    assert_eq!(rows.len(), 7, "7 account records land in membership");
    let by_rk: BTreeMap<String, String> =
        rows.iter().map(|(rk, e)| (rk.clone(), e.clone())).collect();
    let entities: BTreeSet<String> = rows.iter().map(|(_, e)| e.clone()).collect();
    assert_eq!(
        entities.len(),
        4,
        "4 account entities (Acme/Globex/Initech merged, Umbrella alone)"
    );

    // The three same-company pairs (different sources, name-suffix/domain/phone
    // variations) must co-cluster.
    for (a, b) in [
        ("crm:c1", "vendors:v1"),
        ("crm:c2", "vendors:v2"),
        ("crm:c3", "vendors:v3"),
    ] {
        assert_eq!(
            by_rk.get(a),
            by_rk.get(b),
            "{a} and {b} are the same company and must co-cluster"
        );
    }
    // Umbrella (distinct company) must stay separate + be a singleton.
    assert_ne!(
        by_rk.get("crm:c4"),
        by_rk.get("crm:c1"),
        "Umbrella must not merge with Acme"
    );
    let umbrella = by_rk.get("crm:c4").unwrap();
    assert_eq!(
        rows.iter().filter(|(_, e)| e == umbrella).count(),
        1,
        "Umbrella is a singleton entity"
    );

    // Golden: one assembled row per entity, each with a surviving company_name +
    // domain (proves acct_golden_lineage/acct_golden_records assembled).
    let golden: Vec<(String, String)> = session
        .query_strings(
            "select coalesce(company_name, ''), coalesce(domain, '') \
             from lake.main.acct_golden_records order by id",
        )
        .unwrap()
        .into_iter()
        .map(|r| (r[0].clone(), r[1].clone()))
        .collect();
    assert_eq!(golden.len(), 4, "one golden row per account entity");
    assert!(
        golden.iter().all(|(c, d)| !c.is_empty() && !d.is_empty()),
        "every account golden row has a surviving company_name and domain"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}
