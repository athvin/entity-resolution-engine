//! M7 end-to-end gate: drive train -> predict -> cluster entirely through the
//! CLI against a real DuckLake catalog, then re-read the lake with a fresh
//! connection and assert the clusters match the committed golden.
//!
//! Gated behind `DEDUPE_E2E_DUCKLAKE=1` since it installs the `ducklake`
//! extension (network on first run).

use std::path::{Path, PathBuf};

use assert_cmd::Command;
use duckdb::Connection;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn fixtures(p: &str) -> String {
    repo_root()
        .join("fixtures")
        .join(p)
        .to_string_lossy()
        .into_owned()
}

fn enabled() -> bool {
    std::env::var("DEDUPE_E2E_DUCKLAKE").as_deref() == Ok("1")
}

#[test]
fn full_pipeline_through_ducklake() {
    if !enabled() {
        eprintln!("skipping DuckLake e2e (set DEDUPE_E2E_DUCKLAKE=1 to run)");
        return;
    }

    let tmp = std::env::temp_dir().join(format!("dedupe_e2e_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let meta = tmp.join("catalog.ducklake");
    let data = tmp.join("data");
    std::fs::create_dir_all(&data).unwrap();
    let catalog = meta.to_string_lossy().into_owned();
    let data_path = data.to_string_lossy().into_owned();

    // Load fake_1000 into lake.main.people.
    {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("INSTALL ducklake; LOAD ducklake;")
            .unwrap();
        conn.execute_batch(&format!(
            "ATTACH 'ducklake:{catalog}' AS lake (DATA_PATH '{data_path}');"
        ))
        .unwrap();
        conn.execute_batch("CALL lake.set_option('data_inlining_row_limit', 0);")
            .unwrap();
        let csv = fixtures("datasets/fake_1000.csv");
        conn.execute_batch(&format!(
            "CREATE TABLE lake.main.people AS SELECT * FROM read_csv('{csv}', columns = {{'unique_id': 'BIGINT', 'first_name': 'VARCHAR', 'surname': 'VARCHAR', 'dob': 'VARCHAR', 'city': 'VARCHAR', 'email': 'VARCHAR', 'cluster': 'BIGINT'}});"
        ))
        .unwrap();
    }

    let model_path = tmp.join("model.json");
    let settings = fixtures("golden/fake_1000/model_initial.json");

    // train
    Command::cargo_bin("dedupe")
        .unwrap()
        .args([
            "train",
            "--settings",
            &settings,
            "--catalog",
            &catalog,
            "--data-path",
            &data_path,
            "--input",
            "people",
            "--threads",
            "1",
            "--recall",
            "0.7",
            "--max-pairs",
            "1000000",
            "--deterministic-rule",
            r#"(l."first_name" = r."first_name") AND (l."dob" = r."dob")"#,
            "--deterministic-rule",
            "l.first_name = r.first_name and levenshtein(r.surname, l.surname) <= 2",
            "--deterministic-rule",
            r#"l."email" = r."email""#,
            "--em-rule",
            r#"(l."first_name" = r."first_name") AND (l."surname" = r."surname")"#,
            "--em-rule",
            r#"l."dob" = r."dob""#,
            "--output-model",
            model_path.to_str().unwrap(),
        ])
        .assert()
        .success();

    // predict
    Command::cargo_bin("dedupe")
        .unwrap()
        .args([
            "predict",
            "--model",
            model_path.to_str().unwrap(),
            "--catalog",
            &catalog,
            "--data-path",
            &data_path,
            "--input",
            "people",
            "--threads",
            "1",
            "--output",
            "predictions",
        ])
        .assert()
        .success();

    // cluster
    Command::cargo_bin("dedupe")
        .unwrap()
        .args([
            "cluster",
            "--model",
            model_path.to_str().unwrap(),
            "--catalog",
            &catalog,
            "--data-path",
            &data_path,
            "--input",
            "people",
            "--threads",
            "1",
            "--predictions",
            "predictions",
            "--threshold",
            "0.95",
            "--output",
            "clusters",
        ])
        .assert()
        .success();

    // Re-read the lake with a fresh connection and compare cluster membership
    // to the golden.
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("INSTALL ducklake; LOAD ducklake;")
        .unwrap();
    conn.execute_batch(&format!(
        "ATTACH 'ducklake:{catalog}' AS lake (DATA_PATH '{data_path}', READ_ONLY);"
    ))
    .unwrap();
    let golden = fixtures("golden/fake_1000/clusters.parquet");

    let mismatches: i64 = conn
        .query_row(
            &format!(
                "SELECT count(*) FROM lake.main.clusters a \
                 FULL OUTER JOIN read_parquet('{golden}') g ON a.unique_id = g.unique_id \
                 WHERE a.cluster_id IS DISTINCT FROM g.cluster_id"
            ),
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(mismatches, 0, "cluster membership differs from golden");

    let rows: i64 = conn
        .query_row("SELECT count(*) FROM lake.main.clusters", [], |r| r.get(0))
        .unwrap();
    assert_eq!(rows, 1000);

    let _ = std::fs::remove_dir_all(&tmp);
}
