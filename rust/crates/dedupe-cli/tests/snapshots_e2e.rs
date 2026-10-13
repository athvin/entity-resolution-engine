//! DuckLake snapshot / time-travel e2e: write a table twice (two snapshots),
//! list snapshots, and read the table as of each version. Gated behind
//! DEDUPE_E2E_DUCKLAKE=1 (installs the ducklake extension).

use std::path::Path;

use dedupe_core::linker::Linker;
use dedupe_core::session::{Session, SessionOptions, TableHandle};
use dedupe_core::{DuckLakeConfig, LakeWriteOptions, Settings};

fn enabled() -> bool {
    std::env::var("DEDUPE_E2E_DUCKLAKE").as_deref() == Ok("1")
}

const FAKE_1000_SCHEMA: &[(&str, &str)] = &[
    ("unique_id", "BIGINT"),
    ("first_name", "VARCHAR"),
    ("surname", "VARCHAR"),
    ("dob", "VARCHAR"),
    ("city", "VARCHAR"),
    ("email", "VARCHAR"),
    ("cluster", "BIGINT"),
];

#[test]
fn snapshots_and_time_travel() {
    if !enabled() {
        eprintln!("skipping DuckLake snapshot e2e (set DEDUPE_E2E_DUCKLAKE=1 to run)");
        return;
    }

    let tmp = std::env::temp_dir().join(format!("dedupe_snap_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(tmp.join("data")).unwrap();

    let mut session = Session::open_in_memory(SessionOptions::default()).unwrap();
    session
        .attach_ducklake(&DuckLakeConfig {
            metadata: tmp.join("c.ducklake").to_string_lossy().into_owned(),
            data_path: Some(tmp.join("data").to_string_lossy().into_owned()),
            ..Default::default()
        })
        .unwrap();

    // Two writes = two snapshots, with different row counts.
    session
        .execute_batch("CREATE TEMP TABLE stage_v1 AS SELECT * FROM (VALUES (1),(2)) v(x)")
        .unwrap();
    let h1 = TableHandle {
        templated: "stage_v1".into(),
        physical: "stage_v1".into(),
        columns: vec!["x".into()],
    };
    let v1 = session.write_to_lake(&h1, "t").unwrap();

    session
        .execute_batch("CREATE TEMP TABLE stage_v2 AS SELECT * FROM (VALUES (1),(2),(3),(4)) v(x)")
        .unwrap();
    let h2 = TableHandle {
        templated: "stage_v2".into(),
        physical: "stage_v2".into(),
        columns: vec!["x".into()],
    };
    let v2 = session.write_to_lake(&h2, "t").unwrap();

    assert!(
        v2 > v1,
        "second write must create a later snapshot ({v1} -> {v2})"
    );

    let snaps = session.list_snapshots().unwrap();
    assert!(snaps.len() >= 2, "expected at least two snapshots");
    assert_eq!(snaps.last().unwrap().version, v2);

    // Time-travel: the table at v1 has 2 rows, at v2 has 4.
    let at_v1 = session
        .register_lake_table_at_version("t", "t_v1", v1)
        .unwrap();
    let at_v2 = session
        .register_lake_table_at_version("t", "t_v2", v2)
        .unwrap();
    assert_eq!(session.count(&at_v1.physical).unwrap(), 2);
    assert_eq!(session.count(&at_v2.physical).unwrap(), 4);

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn streaming_chunked_predict_matches_direct() {
    if !enabled() {
        eprintln!("skipping DuckLake streaming-predict e2e (set DEDUPE_E2E_DUCKLAKE=1 to run)");
        return;
    }

    let tmp = std::env::temp_dir().join(format!("dedupe_stream_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(tmp.join("data")).unwrap();

    let mut session = Session::open_in_memory(SessionOptions::default()).unwrap();
    session
        .attach_ducklake(&DuckLakeConfig {
            metadata: tmp.join("c.ducklake").to_string_lossy().into_owned(),
            data_path: Some(tmp.join("data").to_string_lossy().into_owned()),
            ..Default::default()
        })
        .unwrap();

    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
    let settings =
        Settings::from_json_file(&fixtures.join("golden/fake_1000/model_final.json")).unwrap();
    let input = session
        .register_csv(
            &fixtures.join("datasets/fake_1000.csv"),
            "input",
            FAKE_1000_SCHEMA,
        )
        .unwrap();
    let linker = Linker::new(&session, settings, &input).unwrap();

    // Stream to a lake table partitioned by match_key, in 3 passes.
    let streamed = linker
        .predict_chunked_to_lake(
            "preds",
            None,
            3,
            &LakeWriteOptions {
                partition_by: vec!["match_key".into()],
                order_by: vec![],
            },
        )
        .unwrap();

    // Direct single-shot predict for comparison.
    let direct = linker.predict(None).unwrap();
    let direct_n = session.count(&direct.physical).unwrap();
    assert_eq!(
        streamed, direct_n,
        "streamed row count must equal direct predict"
    );

    // The streamed lake table must equal the direct output exactly (per-row
    // scoring is deterministic, so match_weight compares bit-for-bit).
    let back = session.register_lake_table("preds", "preds_v").unwrap();
    let diff = session
        .query_scalar_f64(&format!(
            "SELECT count(*)::DOUBLE FROM ( \
               SELECT unique_id_l, unique_id_r, match_key, match_weight FROM {s} \
               EXCEPT \
               SELECT unique_id_l, unique_id_r, match_key, match_weight FROM {d})",
            s = back.physical,
            d = direct.physical,
        ))
        .unwrap() as i64;
    assert_eq!(
        diff, 0,
        "streamed predictions must equal direct predictions"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn maintenance_runs_on_catalog() {
    if !enabled() {
        eprintln!("skipping DuckLake maintenance e2e (set DEDUPE_E2E_DUCKLAKE=1 to run)");
        return;
    }

    let tmp = std::env::temp_dir().join(format!("dedupe_maint_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(tmp.join("data")).unwrap();

    let mut session = Session::open_in_memory(SessionOptions::default()).unwrap();
    session
        .attach_ducklake(&DuckLakeConfig {
            metadata: tmp.join("c.ducklake").to_string_lossy().into_owned(),
            data_path: Some(tmp.join("data").to_string_lossy().into_owned()),
            ..Default::default()
        })
        .unwrap();

    // Options round-trip (DuckLake normalizes `256MB` -> `256000000`).
    session
        .set_lake_option("target_file_size", "256MB")
        .unwrap();
    let opts = session.lake_options().unwrap();
    assert!(
        opts.iter()
            .any(|(k, v)| k == "target_file_size" && !v.is_empty()),
        "set option should be readable back"
    );

    // Several small writes => several data files; then delete to make a delete
    // file, so compaction + rewrite have something to do.
    session
        .execute_batch("CREATE TEMP TABLE s AS SELECT i AS id, i % 5 AS g FROM range(1000) t(i)")
        .unwrap();
    let staged = TableHandle {
        templated: "s".into(),
        physical: "s".into(),
        columns: vec!["id".into(), "g".into()],
    };
    session.write_to_lake(&staged, "m").unwrap();
    session.write_to_lake(&staged, "m").unwrap(); // second snapshot
    session
        .execute_batch("DELETE FROM lake.main.m WHERE id % 10 = 0")
        .unwrap();

    let before = session.lake_table_stats().unwrap();
    assert!(before.iter().any(|(n, _, _)| n == "m"));

    // Each maintenance primitive must run without error.
    session.flush_inlined_data().unwrap();
    session.merge_adjacent_files(None, Some(100)).unwrap();
    session.rewrite_data_files("m").unwrap();
    session
        .set_lake_option("expire_older_than", "0 days")
        .unwrap();
    session.expire_snapshots("0 days").unwrap();
    session.cleanup_old_files().unwrap();
    session.delete_orphaned_files().unwrap();
    session.checkpoint_lake().unwrap();

    // Data is still intact and readable after all maintenance.
    let back = session.register_lake_table("m", "m_view").unwrap();
    let rows = session.count(&back.physical).unwrap();
    assert_eq!(rows, 1000 - 100, "deleted rows should be gone, rest intact");

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn partitioned_write_prunes_by_partition() {
    if !enabled() {
        eprintln!("skipping DuckLake partition e2e (set DEDUPE_E2E_DUCKLAKE=1 to run)");
        return;
    }

    let tmp = std::env::temp_dir().join(format!("dedupe_part_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let data = tmp.join("data");
    std::fs::create_dir_all(&data).unwrap();

    let mut session = Session::open_in_memory(SessionOptions::default()).unwrap();
    session
        .attach_ducklake(&DuckLakeConfig {
            metadata: tmp.join("c.ducklake").to_string_lossy().into_owned(),
            data_path: Some(data.to_string_lossy().into_owned()),
            ..Default::default()
        })
        .unwrap();

    // 400 rows across 4 partition values of `g`, sorted within by `id`.
    session
        .execute_batch("CREATE TEMP TABLE stage AS SELECT i AS id, i % 4 AS g FROM range(400) t(i)")
        .unwrap();
    let staged = TableHandle {
        templated: "stage".into(),
        physical: "stage".into(),
        columns: vec!["id".into(), "g".into()],
    };

    session
        .write_to_lake_with(
            &staged,
            "parted",
            &LakeWriteOptions {
                partition_by: vec!["g".into()],
                order_by: vec!["g".into(), "id".into()],
            },
        )
        .unwrap();

    // Data integrity: all rows present, correct per-partition counts.
    let back = session
        .register_lake_table("parted", "parted_view")
        .unwrap();
    assert_eq!(session.count(&back.physical).unwrap(), 400);
    let per_g = session
        .query_strings(&format!(
            "SELECT g, count(*) FROM {} GROUP BY g ORDER BY g",
            back.physical
        ))
        .unwrap();
    assert_eq!(per_g.len(), 4);
    for row in &per_g {
        assert_eq!(row[1], "100", "each partition should hold 100 rows");
    }

    // One Parquet subtree per partition value proves physical partitioning.
    let subtrees: Vec<_> = std::fs::read_dir(data.join("main").join("parted"))
        .unwrap()
        .filter_map(std::result::Result::ok)
        .filter(|e| e.path().is_dir())
        .filter(|e| e.file_name().to_string_lossy().starts_with("g="))
        .collect();
    assert_eq!(
        subtrees.len(),
        4,
        "expected one Parquet partition subtree per distinct `g` value"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}
