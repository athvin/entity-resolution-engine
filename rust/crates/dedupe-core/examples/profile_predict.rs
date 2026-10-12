//! Profiling / scaling harness for the end-to-end predict pipeline.
//!
//! Runs `predict::run_predict` over a chosen fixture and prints wall-clock
//! time, so it doubles as (a) a quick CPU-scaling check and (b) a target for a
//! sampled profiler such as `samply`.
//!
//! Usage:
//!   cargo run --release --example profile_predict -- [DATASET] [threads] [iters]
//! where DATASET is `fake_1000`, `historical_50k`, or `synthetic[:N]` (a
//! generated N-row set with a large pair count, for scaling/flamegraph work).
//!
//! Capture a flamegraph of the Rust process (native DuckDB frames included):
//!   samply record -- target/release/examples/profile_predict 'synthetic:60000' 8
//! Dump DuckDB's per-operator profile instead:
//!   DEDUPE_PROFILE_SQL=/tmp/p.json \
//!     cargo run --release --example profile_predict -- 'synthetic:40000' 8

use std::path::{Path, PathBuf};
use std::time::Instant;

use dedupe_core::session::{Session, SessionOptions, TableHandle};
use dedupe_core::{Settings, predict};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

/// If `DEDUPE_PROFILE_SQL=<path>` is set, turn on DuckDB's JSON query profiler
/// so each statement's per-operator timing/cardinality tree is written there
/// (the last statement — the scoring pipeline — wins). This is the actionable
/// profiler for the engine: the native DuckDB library is stripped, so a
/// sampled flamegraph only shows addresses, whereas this names each operator.
fn maybe_enable_sql_profiling(session: &Session) {
    if let Ok(path) = std::env::var("DEDUPE_PROFILE_SQL") {
        let p = path.replace('\'', "''");
        session
            .execute_batch(&format!(
                "PRAGMA enable_profiling='json'; PRAGMA profiling_output='{p}';"
            ))
            .expect("enable profiling");
    }
}

/// Settings matching the `synthetic` workload: two comparisons + two blocking
/// rules (one looser, to produce a large scoring load).
fn synthetic_settings() -> Settings {
    let json = r#"{
        "link_type": "dedupe_only",
        "unique_id_column_name": "unique_id",
        "comparisons": [
            {"output_column_name": "first_name", "comparison_levels": [
                {"sql_condition": "first_name_l IS NULL OR first_name_r IS NULL", "is_null_level": true},
                {"sql_condition": "first_name_l = first_name_r", "m_probability": 0.9, "u_probability": 0.01},
                {"sql_condition": "ELSE", "m_probability": 0.1, "u_probability": 0.99}
            ]},
            {"output_column_name": "surname", "comparison_levels": [
                {"sql_condition": "surname_l IS NULL OR surname_r IS NULL", "is_null_level": true},
                {"sql_condition": "surname_l = surname_r", "m_probability": 0.9, "u_probability": 0.02},
                {"sql_condition": "ELSE", "m_probability": 0.1, "u_probability": 0.98}
            ]}
        ],
        "blocking_rules_to_generate_predictions": [
            "l.first_name = r.first_name",
            "l.city = r.city"
        ]
    }"#;
    Settings::from_json_str(json).expect("synthetic settings")
}

fn register_synthetic(session: &Session, n: u64) -> TableHandle {
    session
        .execute_batch(&format!(
            "CREATE TEMP TABLE synth AS SELECT \
               i AS unique_id, \
               'first_' || (i % 2000) AS first_name, \
               'sur_'   || (i % 200)  AS surname, \
               'city_'  || (i % 50)   AS city, \
               '19' || lpad(((i % 80) + 20)::VARCHAR, 2, '0') || '-01-01' AS dob, \
               'user' || (i % 1500) || '@mail.com' AS email \
             FROM range({n}) t(i)"
        ))
        .expect("create synthetic table");
    session
        .register_table("synth", "input")
        .expect("register synthetic")
}

fn main() {
    let mut args = std::env::args().skip(1);
    let dataset = args.next().unwrap_or_else(|| "historical_50k".to_string());
    let threads: Option<u32> = args.next().and_then(|s| s.parse().ok());
    let iters: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(1);

    // Optional memory knobs via env, for stress-testing stability:
    //   DEDUPE_MEMORY_LIMIT=300MB   cap the buffer pool (forces spilling)
    //   DEDUPE_PRESERVE_ORDER=false relax insertion order (DuckDB's OOM remedy)
    let mut opts = match threads {
        Some(n) => SessionOptions::default().with_threads(n),
        None => SessionOptions::default(),
    };
    if let Ok(limit) = std::env::var("DEDUPE_MEMORY_LIMIT") {
        opts = opts.with_memory_limit(limit);
    }
    opts.preserve_insertion_order = match std::env::var("DEDUPE_PRESERVE_ORDER").as_deref() {
        Ok("false") => Some(false),
        Ok("true") => Some(true),
        _ => None,
    };
    let thread_label = threads.map_or_else(|| "all".to_string(), |n| n.to_string());

    // Synthetic mode: `synthetic[:N]` generates an N-row table with looser
    // blocking (a large pair set), so scoring dominates — a meaningful target
    // for a flamegraph and for showing CPU scaling. Default N = 40_000.
    if dataset.starts_with("synthetic") {
        let n: u64 = dataset
            .split_once(':')
            .and_then(|(_, n)| n.parse().ok())
            .unwrap_or(40_000);
        let settings = synthetic_settings();
        for i in 0..iters {
            let session = Session::open_in_memory(opts.clone()).expect("open session");
            maybe_enable_sql_profiling(&session);
            let input = register_synthetic(&session, n);
            let start = Instant::now();
            let predictions =
                predict::run_predict(&session, &settings, &input, None).expect("predict");
            let elapsed = start.elapsed();
            // Skip the trailing count when profiling SQL, so the profiler's
            // output file captures the predict pipeline, not the count query.
            let pairs = if std::env::var_os("DEDUPE_PROFILE_SQL").is_some() {
                -1
            } else {
                session.count(&predictions.physical).expect("count")
            };
            println!(
                "dataset=synthetic(n={n}) threads={thread_label} iter={i} pairs={pairs} predict={:.3}s",
                elapsed.as_secs_f64()
            );
        }
        return;
    }

    let (data_rel, model_rel, uses_parquet) = match dataset.as_str() {
        "fake_1000" => (
            "datasets/fake_1000.csv",
            "golden/fake_1000/model_final.json",
            false,
        ),
        "historical_50k" => (
            "datasets/historical_50k.parquet",
            "golden/historical_50k/model_final.json",
            true,
        ),
        other => {
            eprintln!("unknown dataset `{other}` (use fake_1000 | historical_50k | synthetic[:N])");
            std::process::exit(2);
        }
    };

    let data = fixtures().join(data_rel);
    let model = fixtures().join(model_rel);
    let settings = Settings::from_json_file(&model).expect("load model");

    for i in 0..iters {
        let session = Session::open_in_memory(opts.clone()).expect("open session");
        let input = if uses_parquet {
            let p = data.to_string_lossy().replace('\'', "''");
            session
                .register_table(&format!("read_parquet('{p}')"), "input")
                .expect("register parquet")
        } else {
            session
                .register_csv(
                    &data,
                    "input",
                    &[
                        ("unique_id", "BIGINT"),
                        ("first_name", "VARCHAR"),
                        ("surname", "VARCHAR"),
                        ("dob", "VARCHAR"),
                        ("city", "VARCHAR"),
                        ("email", "VARCHAR"),
                        ("cluster", "BIGINT"),
                    ],
                )
                .expect("register csv")
        };

        let start = Instant::now();
        let predictions = predict::run_predict(&session, &settings, &input, None).expect("predict");
        let elapsed = start.elapsed();
        let n = session.count(&predictions.physical).expect("count");
        println!(
            "dataset={dataset} threads={thread_label} iter={i} pairs={n} predict={:.3}s",
            elapsed.as_secs_f64()
        );
    }
}
