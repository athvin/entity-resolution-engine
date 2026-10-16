//! The `er` CLI — the Rust control-plane entry point (P2), orchestrating the
//! entity-resolution pipeline over a DuckLake lake. Subcommands mirror Python's
//! `er` contract (ingest / standardize / match / reconcile / golden / run-all);
//! each opens the lake and calls the parity-gated stage functions in `er_cli`.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use er_config::{Config, load_config_str};

#[derive(Parser)]
#[command(name = "er", about = "Entity resolution over a DuckLake lake")]
struct Cli {
    /// DuckLake catalog — a `.ducklake` or `sqlite:` path.
    #[arg(long)]
    catalog: String,
    /// Lake Parquet data directory.
    #[arg(long)]
    data_path: String,
    /// The S6 config YAML.
    #[arg(long)]
    config: PathBuf,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// S4.1 — land one source's delivery into raw_records.
    Ingest {
        #[arg(long)]
        source: String,
        #[arg(long)]
        path: PathBuf,
        #[arg(long)]
        full_refresh: bool,
        #[arg(long, default_value = "er-batch")]
        ingest_batch_id: String,
        #[arg(long, default_value = "2026-01-01 00:00:00")]
        ingested_at: String,
    },
    /// S4.2 — build int_std_records. Needs the nickname lexicon seed.
    Standardize {
        #[arg(long)]
        lexicon: PathBuf,
    },
    /// S4.3 — score match_scores against the committed model (full mode).
    Match {
        #[arg(long)]
        model: PathBuf,
        #[arg(long)]
        tf: PathBuf,
        #[arg(long)]
        model_version: String,
        #[arg(long)]
        tf_snapshot_id: String,
        #[arg(long, default_value = "er-run")]
        run_id: String,
        #[arg(long, default_value = "2026-01-01 00:00:00")]
        scored_at: String,
    },
    /// S4.5 — cluster + reconcile into entity_membership/entities/entity_events.
    Reconcile {
        #[arg(long, default_value = "er-run")]
        run_id: String,
        #[arg(long, default_value = "2026-01-01 00:00:00")]
        run_at: String,
    },
    /// S4.6 — assemble golden_records + golden_lineage.
    Golden,
    /// The S4.0 full-mode chain: ingest every `<drop>/<source>.csv` → standardize
    /// → match → reconcile → golden, under one run id.
    RunAll {
        #[arg(long)]
        drop: PathBuf,
        #[arg(long)]
        lexicon: PathBuf,
        #[arg(long)]
        model: PathBuf,
        #[arg(long)]
        tf: PathBuf,
        #[arg(long)]
        model_version: String,
        #[arg(long)]
        tf_snapshot_id: String,
        #[arg(long, default_value = "er-run")]
        run_id: String,
    },
    /// S4.3.2 — train a Splink model on int_std_records; writes the trained
    /// settings JSON + the frozen TF snapshot CSV.
    Train {
        /// Base settings template (comparison + blocking structure).
        #[arg(long)]
        template: PathBuf,
        /// Deterministic blocking rule for the prior (repeatable, SQL).
        #[arg(long = "deterministic-rule")]
        deterministic_rules: Vec<String>,
        /// EM-session blocking rule (repeatable, SQL).
        #[arg(long = "em-rule")]
        em_rules: Vec<String>,
        #[arg(long, default_value_t = 0.9)]
        recall: f64,
        #[arg(long, default_value_t = 1e7)]
        max_pairs: f64,
        #[arg(long)]
        model_version: String,
        #[arg(long)]
        tf_snapshot_id: String,
        /// Where to write the trained settings JSON.
        #[arg(long)]
        out_model: PathBuf,
        /// Where to write the frozen TF snapshot CSV.
        #[arg(long)]
        out_tf: PathBuf,
    },
    /// Apply staged corrections: re-reconcile (assertion-adjusted) + golden over
    /// the existing match_scores, without re-ingesting or re-scoring.
    Correct {
        #[arg(long, default_value = "er-run")]
        run_id: String,
        #[arg(long, default_value = "2026-01-01 00:00:00")]
        run_at: String,
    },
    /// Read-only health report over the lake + config (JSON on stdout).
    Doctor,
    /// Lake maintenance: flush inlined rows, merge small files, expire old
    /// snapshots. Prints a before/after file-count report (JSON).
    Maintain {
        /// Expire snapshots older than this DuckDB interval (e.g. "7 days").
        #[arg(long, default_value = "7 days")]
        expire_older_than: String,
    },
}

fn load_config(path: &PathBuf) -> Result<Config> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    load_config_str(&text).map_err(|e| anyhow::anyhow!("config {}: {e}", path.display()))
}

fn load_lexicon(path: &PathBuf) -> Result<Vec<(String, String)>> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    Ok(text
        .lines()
        .skip(1)
        .filter(|l| !l.is_empty())
        .filter_map(|l| {
            l.split_once(',')
                .map(|(a, b)| (a.to_string(), b.to_string()))
        })
        .collect())
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let config = load_config(&cli.config)?;
    let session = er_cli::open_lake(&cli.catalog, &cli.data_path)?;

    match cli.command {
        Command::Ingest {
            source,
            path,
            full_refresh,
            ingest_batch_id,
            ingested_at,
        } => {
            let csv = std::fs::read_to_string(&path)
                .with_context(|| format!("reading {}", path.display()))?;
            let n = er_cli::run_ingest(
                &session,
                &config,
                &source,
                &csv,
                &ingest_batch_id,
                &ingested_at,
                full_refresh,
            )?;
            println!("ingested {n} rows from {source}");
        }
        Command::Standardize { lexicon } => {
            er_cli::seed_lexicon(&session, &load_lexicon(&lexicon)?)?;
            er_cli::run_standardize(&session, &config)?;
            println!("standardized");
        }
        Command::Match {
            model,
            tf,
            model_version,
            tf_snapshot_id,
            run_id,
            scored_at,
        } => {
            let settings = std::fs::read_to_string(&model)
                .with_context(|| format!("reading {}", model.display()))?;
            er_cli::run_match(
                &session,
                &config,
                &er_cli::MatchModel {
                    settings_json: &settings,
                    tf_csv_path: &tf,
                    model_version: &model_version,
                    tf_snapshot_id: &tf_snapshot_id,
                },
                config.thresholds.review_low,
                &run_id,
                &scored_at,
            )?;
            println!("scored match_scores");
        }
        Command::Reconcile { run_id, run_at } => {
            er_cli::run_reconcile(&session, &config, &run_id, &run_at)?;
            println!("reconciled");
        }
        Command::Golden => {
            er_cli::run_golden(&session, &config)?;
            println!("assembled golden records");
        }
        Command::RunAll {
            drop,
            lexicon,
            model,
            tf,
            model_version,
            tf_snapshot_id,
            run_id,
        } => {
            let lexicon = load_lexicon(&lexicon)?;
            let mut deliveries: Vec<(String, String)> = Vec::new();
            for source in config.sources.keys() {
                let path = drop.join(format!("{source}.csv"));
                if path.exists() {
                    let csv = std::fs::read_to_string(&path)
                        .with_context(|| format!("reading {}", path.display()))?;
                    deliveries.push((source.clone(), csv));
                }
            }
            let settings = std::fs::read_to_string(&model)
                .with_context(|| format!("reading {}", model.display()))?;
            er_cli::run_all(
                &session,
                &config,
                &er_cli::PipelineInputs {
                    lexicon: &lexicon,
                    deliveries: &deliveries,
                    model: &er_cli::MatchModel {
                        settings_json: &settings,
                        tf_csv_path: &tf,
                        model_version: &model_version,
                        tf_snapshot_id: &tf_snapshot_id,
                    },
                    run_id: &run_id,
                    ingested_at: "2026-01-01 00:00:00",
                    scored_at: "2026-01-01 00:00:00",
                    run_at: "2026-01-01 00:00:00",
                },
            )?;
            println!("run-all complete ({run_id})");
        }
        Command::Train {
            template,
            deterministic_rules,
            em_rules,
            recall,
            max_pairs,
            model_version,
            tf_snapshot_id,
            out_model,
            out_tf,
        } => {
            let template_settings_json = std::fs::read_to_string(&template)
                .with_context(|| format!("reading {}", template.display()))?;
            er_cli::run_train(
                &session,
                &er_cli::TrainInputs {
                    template_settings_json: &template_settings_json,
                    deterministic_rules: &deterministic_rules,
                    em_rules: &em_rules,
                    recall,
                    u_max_pairs: max_pairs,
                    model_version: &model_version,
                    tf_snapshot_id: &tf_snapshot_id,
                },
                &out_model,
                &out_tf,
            )?;
            println!(
                "trained {} -> {} + {}",
                model_version,
                out_model.display(),
                out_tf.display()
            );
        }
        Command::Correct { run_id, run_at } => {
            er_cli::run_correct(&session, &config, &run_id, &run_at)?;
            println!("applied corrections ({run_id})");
        }
        Command::Doctor => {
            let report = er_cli::doctor(&session, &config)?;
            println!("{}", serde_json::to_string_pretty(&report)?);
            if report["ok"] != serde_json::Value::Bool(true) {
                std::process::exit(1);
            }
        }
        Command::Maintain { expire_older_than } => {
            let report = er_cli::run_maintain(&session, &expire_older_than)?;
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
    }
    Ok(())
}
