//! Benchmarks for the dedupe pipeline, with a focus on CPU scaling.
//!
//! The engine emits SQL that DuckDB executes, so "use more cores" means
//! letting DuckDB parallelize. These benches vary `SessionOptions::threads`
//! to measure how each stage scales:
//!
//! - `predict_scaling` — the headline: end-to-end predict (blocking + the
//!   vectorized per-pair scoring) over a synthetic dataset, at 1/2/4/all
//!   cores. Scoring is the parallel-friendly part.
//! - `blocking_scaling` — blocking in isolation; dominated by output volume
//!   and block skew, so it scales less than scoring (documented in
//!   PERFORMANCE.md).
//! - `predict_fixture` — end-to-end predict on the committed fake_1000 golden
//!   fixture at a fixed thread count, for regression tracking.
//!
//! Run all: `cargo bench -p dedupe-core`
//! Run one: `cargo bench -p dedupe-core -- predict_scaling`

use std::path::{Path, PathBuf};
use std::time::Duration;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use dedupe_core::session::{Session, SessionOptions, TableHandle};
use dedupe_core::{Settings, predict};

/// Thread counts to sweep: 1, 2, 4, and all logical cores (deduped).
fn thread_sweep() -> Vec<u32> {
    let max = std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get) as u32;
    let mut v: Vec<u32> = [1u32, 2, 4, max]
        .into_iter()
        .filter(|&t| t <= max)
        .collect();
    v.sort_unstable();
    v.dedup();
    v
}

/// A synthetic dataset: `n` rows with a moderate-cardinality `first_name` and
/// low-cardinality `city`, plus `dob`/`email`/`surname` so the golden-shaped
/// comparisons have columns to score. Built in DuckDB from `range(n)`.
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
    // Moderate skew: 2000 first names, 200 surnames, 50 cities.
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

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

fn predict_scaling(c: &mut Criterion) {
    const N: u64 = 40_000;
    let settings = synthetic_settings();
    let mut group = c.benchmark_group("predict_scaling");
    group
        .sample_size(10)
        .measurement_time(Duration::from_secs(12));
    for threads in thread_sweep() {
        let opts = SessionOptions::default().with_threads(threads);
        group.bench_with_input(BenchmarkId::from_parameter(threads), &threads, |b, _| {
            b.iter_batched(
                || {
                    let s = Session::open_in_memory(opts.clone()).unwrap();
                    let input = register_synthetic(&s, N);
                    (s, input)
                },
                |(s, input)| {
                    predict::run_predict(&s, &settings, &input, None).unwrap();
                },
                criterion::BatchSize::PerIteration,
            );
        });
    }
    group.finish();
}

fn blocking_scaling(c: &mut Criterion) {
    const N: u64 = 40_000;
    let settings = synthetic_settings();
    let mut group = c.benchmark_group("blocking_scaling");
    group
        .sample_size(10)
        .measurement_time(Duration::from_secs(10));
    for threads in thread_sweep() {
        let opts = SessionOptions::default().with_threads(threads);
        group.bench_with_input(BenchmarkId::from_parameter(threads), &threads, |b, _| {
            b.iter_batched(
                || {
                    let s = Session::open_in_memory(opts.clone()).unwrap();
                    let input = register_synthetic(&s, N);
                    let concat = predict::materialize_concat(&s, &input).unwrap();
                    (s, concat)
                },
                |(s, concat)| {
                    predict::materialize_blocked_pairs(&s, &settings, &concat).unwrap();
                },
                criterion::BatchSize::PerIteration,
            );
        });
    }
    group.finish();
}

fn predict_fixture(c: &mut Criterion) {
    let csv = fixtures().join("datasets/fake_1000.csv");
    if !csv.exists() {
        return;
    }
    let settings =
        Settings::from_json_file(&fixtures().join("golden/fake_1000/model_final.json")).unwrap();
    let schema = [
        ("unique_id", "BIGINT"),
        ("first_name", "VARCHAR"),
        ("surname", "VARCHAR"),
        ("dob", "VARCHAR"),
        ("city", "VARCHAR"),
        ("email", "VARCHAR"),
        ("cluster", "BIGINT"),
    ];
    let mut group = c.benchmark_group("predict_fixture");
    group.sample_size(20);
    group.bench_function("fake_1000", |b| {
        b.iter_batched(
            || {
                let s = Session::open_in_memory(SessionOptions::deterministic()).unwrap();
                let input = s.register_csv(&csv, "input", &schema).unwrap();
                (s, input)
            },
            |(s, input)| {
                predict::run_predict(&s, &settings, &input, None).unwrap();
            },
            criterion::BatchSize::PerIteration,
        );
    });
    group.finish();
}

criterion_group!(benches, predict_scaling, blocking_scaling, predict_fixture);
criterion_main!(benches);
