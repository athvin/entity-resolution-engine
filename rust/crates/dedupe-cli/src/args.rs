use clap::{Args, Parser, Subcommand};

/// dedupe: a probabilistic record-linkage engine on DuckLake.
#[derive(Parser, Debug)]
#[command(name = "dedupe", version, about)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Train a model (prior -> u -> EM) and write it to JSON.
    Train(TrainArgs),
    /// Predict pairwise match probabilities with a trained model.
    Predict(PredictArgs),
    /// Cluster predictions into entities via connected components.
    Cluster(ClusterArgs),
    /// Incrementally match a new batch of records against an existing dataset
    /// (and within the new batch), using a trained model.
    LinkNew(LinkNewArgs),
    /// Fold a new batch's predictions into an existing cluster assignment
    /// without re-clustering the whole graph.
    ClusterNew(ClusterNewArgs),
    /// Evaluate model accuracy against a ground-truth label column (truth-space
    /// table: precision/recall/F-scores/ROC rates per threshold).
    Accuracy(AccuracyArgs),
    /// Count how many comparisons each blocking rule generates (cumulatively)
    /// vs the total possible — a pre-predict sanity check.
    CountComparisons(CountComparisonsArgs),
    /// List the DuckLake catalog's snapshots (version + timestamp).
    Snapshots(SnapshotsArgs),
    /// Run DuckLake catalog maintenance: compaction, snapshot expiry, delete-
    /// file rewrite, orphan cleanup, and option setting.
    Maintenance(MaintenanceArgs),
    /// Strict rule-based linkage: candidate pairs from blocking rules, no
    /// probabilistic scoring.
    DeterministicLink(DeterministicLinkArgs),
    /// Profile input columns: completeness (non-null proportion) per column.
    Profile(ProfileArgs),
}

/// How to locate the input records.
#[derive(Args, Debug)]
pub struct InputSource {
    /// DuckLake ATTACH payload (everything after `ducklake:`), e.g.
    /// `catalog.ducklake`, `sqlite:meta.db`, `postgres:dbname=lake host=...`.
    #[arg(long)]
    pub catalog: Option<String>,
    /// DATA_PATH for the lake (where Parquet data lives).
    #[arg(long)]
    pub data_path: Option<String>,
    /// Schema within the lake catalog.
    #[arg(long, default_value = "main")]
    pub schema: String,
    /// Input table name within the lake catalog.
    #[arg(long)]
    pub input: Option<String>,
    /// Read input directly from a CSV file instead of the lake.
    #[arg(long)]
    pub input_csv: Option<String>,
    /// Read input directly from a Parquet file instead of the lake.
    #[arg(long)]
    pub input_parquet: Option<String>,
    /// Pin DuckDB thread count (use 1 for bit-reproducible float aggregation;
    /// omit to use all CPU cores).
    #[arg(long)]
    pub threads: Option<u32>,
    /// In-memory buffer cap, e.g. `8GB` or `75%`. Beyond it DuckDB spills to
    /// `--temp-dir` instead of OOM-ing.
    #[arg(long)]
    pub memory_limit: Option<String>,
    /// Directory for spilling intermediates larger than `--memory-limit`.
    /// Set this to run workloads larger than RAM.
    #[arg(long)]
    pub temp_dir: Option<String>,
    /// DuckDB extension to LOAD so its functions are usable in comparison /
    /// blocking SQL, e.g. `rapidfuzz` or `splink_udfs` (repeatable). Core
    /// metrics (jaro_winkler, levenshtein, jaccard) need no extension.
    #[arg(long = "extension")]
    pub extensions: Vec<String>,
    /// Directory holding pre-installed extension binaries (bake into the image
    /// to avoid any runtime download). DuckDB `extension_directory`.
    #[arg(long)]
    pub extension_dir: Option<String>,
    /// `INSTALL ... FROM community` before loading extensions (needs network;
    /// for local/dev use). Default: LOAD only, assuming pre-baked binaries.
    #[arg(long)]
    pub install_extensions: bool,
    /// Link-type input dataset as NAME=REF (repeatable). REF is a lake table,
    /// or a path ending in .parquet/.csv. Use for link_only/link_and_dedupe;
    /// each NAME becomes a source_dataset. When given, overrides --input.
    #[arg(long = "dataset")]
    pub datasets: Vec<String>,
    /// Read lake inputs as of this snapshot version (DuckLake time travel).
    #[arg(long)]
    pub as_of_version: Option<i64>,
    /// Partition the output lake table by these columns (repeatable). Writes
    /// one Parquet subtree per value so filtered reads skip non-matching
    /// files. DuckLake has no secondary indexes; this + sort-on-write are the
    /// pruning levers. Good choices: source_dataset, a batch/date column.
    #[arg(long = "partition-by")]
    pub partition_by: Vec<String>,
    /// Sort the output lake table by these columns on write (repeatable).
    /// Tightens each Parquet file's min/max statistics — DuckLake's
    /// clustering-key equivalent.
    #[arg(long = "order-by")]
    pub order_by: Vec<String>,
}

impl InputSource {
    /// Physical layout options for writing results back to the lake.
    #[must_use]
    pub fn lake_write_options(&self) -> dedupe_core::LakeWriteOptions {
        dedupe_core::LakeWriteOptions {
            partition_by: self.partition_by.clone(),
            order_by: self.order_by.clone(),
        }
    }
}

#[derive(Args, Debug)]
pub struct TrainArgs {
    /// Settings JSON.
    #[arg(long)]
    pub settings: String,
    #[command(flatten)]
    pub source: InputSource,
    /// Deterministic matching rule (repeatable) for prior estimation.
    #[arg(long = "deterministic-rule")]
    pub deterministic_rules: Vec<String>,
    /// Estimated recall of the deterministic rules.
    #[arg(long, default_value_t = 0.7)]
    pub recall: f64,
    /// Target pair count for u estimation sampling.
    #[arg(long, default_value_t = 1e6)]
    pub max_pairs: f64,
    /// EM training blocking rule (repeatable).
    #[arg(long = "em-rule")]
    pub em_rules: Vec<String>,
    /// Path to write the trained model JSON.
    #[arg(long)]
    pub output_model: String,
}

#[derive(Args, Debug)]
pub struct PredictArgs {
    /// Trained model JSON.
    #[arg(long)]
    pub model: String,
    #[command(flatten)]
    pub source: InputSource,
    /// Output table name written to the lake.
    #[arg(long, default_value = "predictions")]
    pub output: String,
    /// Optional match-probability threshold filter.
    #[arg(long)]
    pub threshold_match_probability: Option<f64>,
    /// Also write output to this Parquet file (when not using a lake).
    #[arg(long)]
    pub output_parquet: Option<String>,
    /// Score the candidate pairs in this many bounded-memory hash passes
    /// (default 1 = single shot). Raise it when the pair set is too large to
    /// score at once; the result is identical. See PERFORMANCE.md.
    #[arg(long, default_value_t = 1)]
    pub num_chunks: u32,
    /// Stream each chunk straight to the lake output table instead of
    /// accumulating the full result first — constant memory end to end.
    /// Requires --catalog; each pass commits a snapshot (compact afterwards
    /// with `dedupe maintenance --checkpoint`).
    #[arg(long)]
    pub stream: bool,
}

#[derive(Args, Debug)]
pub struct LinkNewArgs {
    /// Trained model JSON.
    #[arg(long)]
    pub model: String,
    /// How to reach the existing dataset + (via its lake/files) the catalog.
    /// `--input` names the existing table; the new batch is given separately.
    #[command(flatten)]
    pub source: InputSource,
    /// New-batch records as a lake table name.
    #[arg(long)]
    pub new_input: Option<String>,
    /// New-batch records as a CSV file.
    #[arg(long)]
    pub new_input_csv: Option<String>,
    /// New-batch records as a Parquet file.
    #[arg(long)]
    pub new_input_parquet: Option<String>,
    /// What to score: new-vs-existing, within-new, or both (default).
    #[arg(long, value_enum, default_value_t = LinkNewScope::Both)]
    pub scope: LinkNewScope,
    /// Optional match-probability threshold filter.
    #[arg(long)]
    pub threshold_match_probability: Option<f64>,
    /// Output table name written to the lake.
    #[arg(long, default_value = "incremental_predictions")]
    pub output: String,
    /// Also write output to this Parquet file (when not using a lake).
    #[arg(long)]
    pub output_parquet: Option<String>,
}

#[derive(Args, Debug)]
pub struct ClusterNewArgs {
    /// Trained model JSON (for id/settings context).
    #[arg(long)]
    pub model: String,
    /// All records (existing + new) — provides the node set.
    #[command(flatten)]
    pub source: InputSource,
    /// Existing cluster assignment as a lake table (needs unique_id +
    /// cluster_id columns).
    #[arg(long)]
    pub existing_clusters: Option<String>,
    /// Existing cluster assignment as a Parquet file.
    #[arg(long)]
    pub existing_clusters_parquet: Option<String>,
    /// New-batch predictions as a lake table.
    #[arg(long)]
    pub predictions: Option<String>,
    /// New-batch predictions as a Parquet file.
    #[arg(long)]
    pub predictions_parquet: Option<String>,
    /// Cluster at this match-probability threshold.
    #[arg(long, default_value_t = 0.95)]
    pub threshold: f64,
    /// Output table name written to the lake.
    #[arg(long, default_value = "clusters")]
    pub output: String,
    /// Also write output to this Parquet file (when not using a lake).
    #[arg(long)]
    pub output_parquet: Option<String>,
}

#[derive(Args, Debug)]
pub struct SnapshotsArgs {
    #[command(flatten)]
    pub source: InputSource,
}

#[derive(Args, Debug)]
pub struct MaintenanceArgs {
    #[command(flatten)]
    pub source: InputSource,
    /// Run a full CHECKPOINT (flush inlined -> expire -> merge -> rewrite ->
    /// cleanup -> delete orphans), bounded by the catalog options below.
    #[arg(long)]
    pub checkpoint: bool,
    /// Flush inlined (catalog-resident) rows out to Parquet.
    #[arg(long)]
    pub flush_inlined: bool,
    /// Merge small adjacent data files (tiered compaction).
    #[arg(long)]
    pub merge_adjacent: bool,
    /// Max output file size (bytes) for --merge-adjacent.
    #[arg(long)]
    pub max_file_size: Option<u64>,
    /// Max files compacted per table for --merge-adjacent (bounds memory).
    #[arg(long)]
    pub max_compacted_files: Option<u64>,
    /// Rewrite delete-heavy files for this table (repeatable).
    #[arg(long = "rewrite-table")]
    pub rewrite_tables: Vec<String>,
    /// Delete files unreferenced by live snapshots.
    #[arg(long)]
    pub cleanup_old_files: bool,
    /// Delete files in the data path unknown to the catalog.
    #[arg(long)]
    pub delete_orphaned_files: bool,
    /// Set `expire_older_than` (e.g. `7 days`) before running.
    #[arg(long)]
    pub expire_older_than: Option<String>,
    /// Set `delete_older_than` (e.g. `2 days`) before running.
    #[arg(long)]
    pub delete_older_than: Option<String>,
    /// Set `target_file_size` (e.g. `512MB`) before running.
    #[arg(long)]
    pub target_file_size: Option<String>,
    /// Set an arbitrary catalog option as KEY=VALUE (repeatable).
    #[arg(long = "set")]
    pub set_options: Vec<String>,
    /// Print all catalog options and exit.
    #[arg(long)]
    pub list_options: bool,
}

#[derive(Args, Debug)]
pub struct ProfileArgs {
    /// Settings or model JSON (id columns / link type).
    #[arg(long)]
    pub settings: String,
    #[command(flatten)]
    pub source: InputSource,
    /// Columns to profile (repeatable).
    #[arg(long = "column")]
    pub columns: Vec<String>,
}

#[derive(Args, Debug)]
pub struct DeterministicLinkArgs {
    /// Settings or model JSON (blocking rules + id columns).
    #[arg(long)]
    pub settings: String,
    #[command(flatten)]
    pub source: InputSource,
    /// Output table name written to the lake.
    #[arg(long, default_value = "deterministic_pairs")]
    pub output: String,
    /// Also write output to this Parquet file (when not using a lake).
    #[arg(long)]
    pub output_parquet: Option<String>,
}

#[derive(Args, Debug)]
pub struct CountComparisonsArgs {
    /// Settings or model JSON (for link_type / unique id context).
    #[arg(long)]
    pub settings: String,
    #[command(flatten)]
    pub source: InputSource,
    /// Blocking rule SQL to count (repeatable; counted cumulatively in order).
    #[arg(long = "blocking-rule")]
    pub blocking_rules: Vec<String>,
}

#[derive(Args, Debug)]
pub struct AccuracyArgs {
    /// Trained model JSON.
    #[arg(long)]
    pub model: String,
    #[command(flatten)]
    pub source: InputSource,
    /// Ground-truth label column (equal values = a true match).
    #[arg(long)]
    pub label_column: String,
    /// Round match weights to this granularity when enumerating thresholds.
    #[arg(long, default_value_t = 0.1)]
    pub match_weight_round_to_nearest: f64,
    /// Output table name written to the lake.
    #[arg(long, default_value = "accuracy_truth_space")]
    pub output: String,
    /// Also write output to this Parquet file (when not using a lake).
    #[arg(long)]
    pub output_parquet: Option<String>,
}

#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq)]
pub enum LinkNewScope {
    /// Pairs between the new batch and the existing dataset.
    Between,
    /// Pairs within the new batch.
    Within,
    /// Both, unioned (full incremental dedupe of the new batch).
    Both,
}

#[derive(Args, Debug)]
pub struct ClusterArgs {
    /// Trained model JSON (for id/settings context).
    #[arg(long)]
    pub model: String,
    #[command(flatten)]
    pub source: InputSource,
    /// Predictions table name in the lake (or use --predictions-parquet).
    #[arg(long)]
    pub predictions: Option<String>,
    /// Predictions Parquet file.
    #[arg(long)]
    pub predictions_parquet: Option<String>,
    /// Cluster at this match-probability threshold.
    #[arg(long, default_value_t = 0.95)]
    pub threshold: f64,
    /// Output table name written to the lake.
    #[arg(long, default_value = "clusters")]
    pub output: String,
    /// Also write output to this Parquet file (when not using a lake).
    #[arg(long)]
    pub output_parquet: Option<String>,
}
