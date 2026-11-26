//! #1230: an export artifact over a real lake (behind `DEDUPE_E2E_DUCKLAKE=1`).
//!
//! Two full runs over the account fixture, the second adding one company, so
//! run R has four golden records and three duplicate groups and run R+1 has
//! five golden records. Each case writes with a page size of 2, so every
//! export of more than two rows crosses a page boundary:
//!
//! 1. golden CSV at R's snapshot: one header, four rows, even though R+1 has
//!    committed since: the export reads its pinned snapshot, not the latest;
//! 2. golden JSON Lines at the latest: five lines;
//! 3. duplicates JSON Lines at R: three groups, each with its members;
//! 4. an empty lake (the snapshot before any run): a zero-row artifact, not an
//!    error, so "nothing yet" and "failed" stay different answers;
//! 5. the row ceiling: the export fails naming it, and leaves no file and no
//!    `.partial` behind, so a failed export can never be downloaded;
//! 6. lineage (#1231), the join a customer makes back to their own keys: every
//!    source record of a file-ingested (non-Salesforce) dataset appears exactly
//!    once, and the entities it names are exactly the golden export's;
//! 7. Parquet (#1241): golden at R round-trips through a reader with exactly
//!    `AcctGoldenRecord`'s columns and R's four entities, across a page
//!    boundary; an empty export is still a readable file with that schema;
//!    lineage carries the same seven rows as its CSV.

use std::path::{Path, PathBuf};

use arrow::array::Array as _;
use arrow::record_batch::RecordBatchReader as _;

use er_config::{EntityType, load_config_str};
use er_server::exports::{self, ExportFormat, ExportKind, ExportRequest};
use er_server::read;

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

fn request(kind: ExportKind, format: ExportFormat, snapshot: i64) -> ExportRequest {
    ExportRequest {
        kind,
        format,
        snapshot: Some(snapshot),
        tenant_prefix: None,
    }
}

/// Read a Parquet artifact back: its column names and every row's batch.
fn parquet(path: &Path) -> (Vec<String>, Vec<arrow::record_batch::RecordBatch>) {
    let file = std::fs::File::open(path).unwrap();
    let reader = parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder::try_new(file)
        .unwrap()
        .build()
        .unwrap();
    let columns = reader
        .schema()
        .fields()
        .iter()
        .map(|f| f.name().clone())
        .collect();
    (columns, reader.map(Result::unwrap).collect())
}

fn strings(batches: &[arrow::record_batch::RecordBatch], column: &str) -> Vec<String> {
    batches
        .iter()
        .flat_map(|b| {
            let array = b.column_by_name(column).unwrap();
            let array = arrow::compute::cast(array, &arrow::datatypes::DataType::Utf8).unwrap();
            let array = array
                .as_any()
                .downcast_ref::<arrow::array::StringArray>()
                .unwrap()
                .clone();
            (0..array.len())
                .map(|i| array.value(i).to_string())
                .collect::<Vec<_>>()
        })
        .collect()
}

fn lines(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect()
}

#[test]
fn an_export_writes_every_page_of_its_pinned_snapshot() {
    if !enabled() {
        eprintln!("skipping export e2e (set DEDUPE_E2E_DUCKLAKE=1 to run)");
        return;
    }
    let tmp = std::env::temp_dir().join(format!("er_export_write_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(tmp.join("data")).unwrap();
    let catalog = tmp.join("catalog.ducklake").to_string_lossy().into_owned();
    let data_path = tmp.join("data").to_string_lossy().into_owned();
    let session = er_cli::open_lake(&catalog, &data_path).unwrap();
    let empty = read::latest_snapshot(&session).unwrap();
    let config =
        load_config_str(&std::fs::read_to_string(repo_fixture("acct_base/config.yaml")).unwrap())
            .unwrap();
    let crm = std::fs::read_to_string(repo_fixture("acct_base/crm.csv")).unwrap();
    let vendors = std::fs::read_to_string(repo_fixture("acct_base/vendors.csv")).unwrap();
    let model_json = std::fs::read_to_string(repo_fixture("acct_model_v1.json")).unwrap();
    let tf_csv = repo_fixture("acct_model_v1.tf.csv");
    let run = |run_id: &str, day: u8, crm: &str| {
        let deliveries = vec![
            ("crm".to_string(), crm.to_string()),
            ("vendors".to_string(), vendors.clone()),
        ];
        er_cli::run_all(
            &session,
            &config,
            &er_cli::PipelineInputs {
                mode: "full",
                rebuild_reason: None,
                segment: None,
                lexicon: &Vec::new(),
                ignore_words: &[],
                deliveries: &deliveries,
                model: &er_cli::MatchModel {
                    settings_json: &model_json,
                    tf_csv_path: Path::new(&tf_csv),
                    model_version: "acct_v1",
                    tf_snapshot_id: "ACCTTFSNAP0000000000000001",
                },
                run_id,
                ingested_at: &format!("2026-03-{day:02} 00:00:00"),
                scored_at: &format!("2026-03-{day:02} 01:00:00"),
                run_at: &format!("2026-03-{day:02} 02:00:00"),
            },
        )
        .unwrap();
    };
    run("01EXPORTR00000000000000000", 1, &crm);
    let r = read::latest_snapshot(&session).unwrap();
    let more = crm.trim_end_matches('\n').to_string()
        + "\nc9,Hooli Systems,https://hooli.example,212-555-0999,99 Unique Way,Faraway,WA,98001,2026-01-09\n";
    run("01EXPORTR10000000000000000", 5, &more);
    let latest = read::latest_snapshot(&session).unwrap();
    let out = tmp.join("exports");
    let write = |name: &str, req: &ExportRequest, max_rows: usize| {
        let path = out.join(name);
        (
            exports::write(&session, EntityType::Account, req, &path, 2, max_rows),
            path,
        )
    };

    // 1. Golden CSV at R: four rows and one header, across two pages, after
    //    R+1 committed.
    let (report, path) = write(
        "r.csv",
        &request(ExportKind::Golden, ExportFormat::Csv, r),
        exports::MAX_ROWS,
    );
    assert_eq!(report.unwrap().rows, 4);
    let csv = lines(&path);
    assert_eq!(csv.len(), 5, "one header and four rows: {csv:?}");
    assert!(
        csv[0].contains("company_name"),
        "the header names the columns: {}",
        csv[0]
    );
    assert_eq!(
        csv.iter().filter(|l| l.contains("company_name")).count(),
        1,
        "a page boundary must not repeat the header"
    );

    // 2. Golden JSON Lines at the latest: R+1's five.
    let (report, path) = write(
        "latest.jsonl",
        &request(ExportKind::Golden, ExportFormat::Jsonl, latest),
        exports::MAX_ROWS,
    );
    assert_eq!(report.unwrap().rows, 5);
    let golden = lines(&path);
    assert_eq!(golden.len(), 5);
    for line in &golden {
        let row: serde_json::Value = serde_json::from_str(line).unwrap();
        assert!(row.get("company_name").is_some(), "{line}");
    }

    // 3. Duplicate groups at R: three, each naming its members.
    let (report, path) = write(
        "dups.jsonl",
        &request(ExportKind::Duplicates, ExportFormat::Jsonl, r),
        exports::MAX_ROWS,
    );
    assert_eq!(report.unwrap().rows, 3);
    for line in lines(&path) {
        let group: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(group["members"].as_array().map(Vec::len), Some(2), "{line}");
    }

    // 4. Before any run: an empty artifact, not a failure.
    let (report, path) = write(
        "empty.jsonl",
        &request(ExportKind::Golden, ExportFormat::Jsonl, empty),
        exports::MAX_ROWS,
    );
    assert_eq!(report.unwrap().rows, 0);
    assert_eq!(lines(&path).len(), 0);

    // 5. Past the ceiling: a named failure and nothing left behind.
    let (report, path) = write(
        "ceiling.csv",
        &request(ExportKind::Golden, ExportFormat::Csv, latest),
        3,
    );
    assert!(report.unwrap_err().contains("3-row ceiling"));
    assert!(!path.exists(), "a failed export left its artifact");
    let leftovers: Vec<_> = std::fs::read_dir(&out)
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".partial"))
        .collect();
    assert!(leftovers.is_empty(), "partial files left: {leftovers:?}");

    // 6. Lineage at R, as CSV and JSONL: the seven source records (four crm,
    //    three vendors), each once, joined to R's four golden records.
    let (report, path) = write(
        "lineage.csv",
        &request(ExportKind::Lineage, ExportFormat::Csv, r),
        exports::MAX_ROWS,
    );
    assert_eq!(report.unwrap().rows, 7);
    let lineage = lines(&path);
    assert_eq!(
        lineage[0],
        "record_key,source_system,source_record_id,entity_id"
    );
    let rows: Vec<Vec<&str>> = lineage[1..]
        .iter()
        .map(|l| l.split(',').collect())
        .collect();
    let mut sources: Vec<(String, String)> = rows
        .iter()
        .map(|r| (r[1].to_string(), r[2].to_string()))
        .collect();
    sources.sort();
    let mut expected: Vec<(String, String)> = crm
        .lines()
        .skip(1)
        .map(|l| ("crm".to_string(), l.split(',').next().unwrap().to_string()))
        .chain(vendors.lines().skip(1).map(|l| {
            (
                "vendors".to_string(),
                l.split(',').next().unwrap().to_string(),
            )
        }))
        .collect();
    expected.sort();
    assert_eq!(sources, expected, "every source record, exactly once");
    let entities: std::collections::BTreeSet<&str> = rows.iter().map(|r| r[3]).collect();
    let (report, path) = write(
        "golden-r.jsonl",
        &request(ExportKind::Golden, ExportFormat::Jsonl, r),
        exports::MAX_ROWS,
    );
    assert_eq!(report.unwrap().rows, 4);
    let golden: std::collections::BTreeSet<String> = lines(&path)
        .iter()
        .map(|l| {
            serde_json::from_str::<serde_json::Value>(l).unwrap()["id"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect();
    assert_eq!(
        entities,
        golden.iter().map(String::as_str).collect(),
        "the lineage's entities are exactly the golden records"
    );
    let (report, path) = write(
        "lineage.jsonl",
        &request(ExportKind::Lineage, ExportFormat::Jsonl, r),
        exports::MAX_ROWS,
    );
    assert_eq!(report.unwrap().rows, 7);
    assert_eq!(lines(&path).len(), 7);

    // 7. Parquet. Golden at R: the entity's declared columns, R's four ids.
    let declared: Vec<String> = er_entities::acct_golden_record::AcctGoldenRecord::arrow_schema()
        .fields()
        .iter()
        .map(|f| f.name().clone())
        .collect();
    let (report, path) = write(
        "golden-r.parquet",
        &request(ExportKind::Golden, ExportFormat::Parquet, r),
        exports::MAX_ROWS,
    );
    assert_eq!(report.unwrap().rows, 4);
    let (columns, batches) = parquet(&path);
    assert_eq!(columns, declared, "the GoldenRecord column set");
    let ids: std::collections::BTreeSet<String> = strings(&batches, "id").into_iter().collect();
    assert_eq!(ids, golden, "the same four entities as the JSONL export");
    // An empty export is a readable, zero-row file with that schema.
    let (report, path) = write(
        "empty.parquet",
        &request(ExportKind::Golden, ExportFormat::Parquet, empty),
        exports::MAX_ROWS,
    );
    assert_eq!(report.unwrap().rows, 0);
    let (columns, batches) = parquet(&path);
    assert_eq!(columns, declared);
    assert_eq!(
        batches
            .iter()
            .map(arrow::record_batch::RecordBatch::num_rows)
            .sum::<usize>(),
        0
    );
    // Lineage: the CSV's columns and its seven rows.
    let (report, path) = write(
        "lineage.parquet",
        &request(ExportKind::Lineage, ExportFormat::Parquet, r),
        exports::MAX_ROWS,
    );
    assert_eq!(report.unwrap().rows, 7);
    let (columns, batches) = parquet(&path);
    assert_eq!(columns.join(","), lineage[0]);
    let mut from_parquet: Vec<String> = (0..7)
        .map(|i| {
            [
                "record_key",
                "source_system",
                "source_record_id",
                "entity_id",
            ]
            .map(|c| strings(&batches, c)[i].clone())
            .join(",")
        })
        .collect();
    let mut from_csv: Vec<String> = lineage[1..].to_vec();
    from_parquet.sort();
    from_csv.sort();
    assert_eq!(from_parquet, from_csv);
    // Duplicates are JSON Lines only.
    assert!(
        ExportRequest::from_params(&serde_json::json!({"kind": "duplicates", "format": "parquet"}))
            .is_err()
    );

    let _ = std::fs::remove_dir_all(&tmp);
}
