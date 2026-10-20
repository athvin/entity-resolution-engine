mod args;

use std::path::Path;

use anyhow::{Context, Result, bail};
use args::{
    AccuracyArgs, Cli, ClusterArgs, ClusterNewArgs, Command, CountComparisonsArgs,
    DeterministicLinkArgs, InputSource, LinkNewArgs, LinkNewScope, MaintenanceArgs, PredictArgs,
    ProfileArgs, SnapshotsArgs, TrainArgs,
};
use clap::Parser;
use dedupe_core::accuracy::{AccuracyOptions, truth_space_from_labels_column};
use dedupe_core::blocking_analysis::count_comparisons_from_blocking_rules;
use dedupe_core::clustering::{ClusterOptions, clustered_with_input_data, incremental_cluster};
use dedupe_core::incremental::{predict_between, predict_within};
use dedupe_core::linker::{Linker, TrainConfig};
use dedupe_core::predict;
use dedupe_core::profile;
use dedupe_core::session::TableHandle;
use dedupe_core::{DuckLakeConfig, Session, SessionOptions, Settings};

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    match Cli::parse().command {
        Command::Train(a) => train(a),
        Command::Predict(a) => predict(a),
        Command::Cluster(a) => cluster(a),
        Command::LinkNew(a) => link_new(a),
        Command::ClusterNew(a) => cluster_new(a),
        Command::Accuracy(a) => accuracy(a),
        Command::CountComparisons(a) => count_comparisons(a),
        Command::Snapshots(a) => snapshots(a),
        Command::Maintenance(a) => maintenance(a),
        Command::DeterministicLink(a) => deterministic_link(a),
        Command::Profile(a) => profile_columns(a),
    }
}

fn profile_columns(a: ProfileArgs) -> Result<()> {
    if a.columns.is_empty() {
        bail!("profile requires at least one --column");
    }
    let settings = Settings::from_json_file(Path::new(&a.settings)).context("loading settings")?;
    let session = open_session(&a.source)?;
    let inputs = resolve_inputs(&session, &a.source)?;
    let refs: Vec<(&TableHandle, &str)> = inputs.iter().map(|(h, n)| (h, n.as_str())).collect();
    let concat = predict::materialize_concat_multi(&session, &refs, &settings)?;
    let comp = profile::completeness(&session, &settings, &concat, &a.columns, "input_data_1")?;
    let rows = session.query_strings(&format!(
        "select source_dataset, column_name, total_null_rows, total_rows_inc_nulls, completeness from {} order by column_name",
        comp.physical
    ))?;
    for r in &rows {
        eprintln!(
            "{} / {}: {}/{} non-null (completeness {})",
            r[0],
            r[1],
            r[3].parse::<i64>().unwrap_or(0) - r[2].parse::<i64>().unwrap_or(0),
            r[3],
            r[4],
        );
    }
    println!("{}", rows.len());
    Ok(())
}

fn deterministic_link(a: DeterministicLinkArgs) -> Result<()> {
    let settings = Settings::from_json_file(Path::new(&a.settings)).context("loading settings")?;
    let session = open_session(&a.source)?;
    let inputs = resolve_inputs(&session, &a.source)?;
    let refs: Vec<(&TableHandle, &str)> = inputs.iter().map(|(h, n)| (h, n.as_str())).collect();
    let concat = predict::materialize_concat_multi(&session, &refs, &settings)?;
    let tf = predict::materialize_tf_tables(&session, &settings, &concat)?;
    let pairs = predict::deterministic_link(&session, &settings, &concat, &tf)?;
    let n = session.count(&pairs.physical)?;

    if let Some(path) = &a.output_parquet {
        write_parquet(&session, &pairs, path)?;
    }
    if a.source.catalog.is_some() {
        session.write_to_lake_with(&pairs, &a.output, &a.source.lake_write_options())?;
        eprintln!("wrote {n} deterministic pairs to lake table `{}`", a.output);
    } else if a.output_parquet.is_none() {
        bail!("no output target: provide --catalog (lake) or --output-parquet");
    }
    println!("{n}");
    Ok(())
}

fn snapshots(a: SnapshotsArgs) -> Result<()> {
    if a.source.catalog.is_none() {
        bail!("snapshots requires --catalog");
    }
    let session = open_session(&a.source)?;
    let snaps = session.list_snapshots()?;
    for s in &snaps {
        eprintln!("version {} at {}", s.version, s.timestamp);
    }
    let json: Vec<_> = snaps
        .iter()
        .map(|s| serde_json::json!({"version": s.version, "timestamp": s.timestamp}))
        .collect();
    println!("{}", serde_json::to_string(&json)?);
    Ok(())
}

fn maintenance(a: MaintenanceArgs) -> Result<()> {
    if a.source.catalog.is_none() {
        bail!("maintenance requires --catalog (a DuckLake catalog)");
    }
    let session = open_session(&a.source)?;

    if a.list_options {
        for (name, value) in session.lake_options()? {
            println!("{name} = {value}");
        }
        return Ok(());
    }

    // Apply catalog options first (they steer CHECKPOINT's expire/merge steps).
    if let Some(v) = &a.expire_older_than {
        session.set_lake_option("expire_older_than", v)?;
    }
    if let Some(v) = &a.delete_older_than {
        session.set_lake_option("delete_older_than", v)?;
    }
    if let Some(v) = &a.target_file_size {
        session.set_lake_option("target_file_size", v)?;
    }
    for kv in &a.set_options {
        let (k, v) = kv
            .split_once('=')
            .with_context(|| format!("--set must be KEY=VALUE, got `{kv}`"))?;
        session.set_lake_option(k, v)?;
    }

    let before = session.lake_table_stats()?;

    // Individual steps (skipped by a full checkpoint, which runs them all).
    let mut ran: Vec<String> = Vec::new();
    if a.checkpoint {
        session.checkpoint_lake()?;
        ran.push("checkpoint".into());
    } else {
        if a.flush_inlined {
            session.flush_inlined_data()?;
            ran.push("flush-inlined".into());
        }
        if a.merge_adjacent {
            session.merge_adjacent_files(a.max_file_size, a.max_compacted_files)?;
            ran.push("merge-adjacent".into());
        }
        for table in &a.rewrite_tables {
            session.rewrite_data_files(table)?;
            ran.push(format!("rewrite:{table}"));
        }
        if let Some(v) = &a.expire_older_than {
            session.expire_snapshots(v)?;
            ran.push("expire-snapshots".into());
        }
        if a.cleanup_old_files {
            session.cleanup_old_files()?;
            ran.push("cleanup-old-files".into());
        }
        if a.delete_orphaned_files {
            session.delete_orphaned_files()?;
            ran.push("delete-orphaned-files".into());
        }
    }

    if ran.is_empty() {
        bail!(
            "no maintenance action requested; pass --checkpoint or one of \
             --flush-inlined / --merge-adjacent / --rewrite-table / \
             --cleanup-old-files / --delete-orphaned-files (or --list-options)"
        );
    }

    let after = session.lake_table_stats()?;
    eprintln!("ran: {}", ran.join(", "));
    for (name, fc, dc) in &after {
        let (bf, bd) = before
            .iter()
            .find(|(n, _, _)| n == name)
            .map_or((0, 0), |(_, f, d)| (*f, *d));
        eprintln!("  {name}: files {bf}->{fc}, delete_files {bd}->{dc}");
    }
    println!("{}", ran.len());
    Ok(())
}

/// Open a session, attaching a lake if the source references one.
fn open_session(source: &InputSource) -> Result<Session> {
    // Memory-stable production defaults: when running multi-threaded (the
    // scaling case, already non-deterministic in float-sum order), relax
    // insertion-order preservation — DuckDB's own OOM remedy — so large scans
    // and writes spill cleanly. With `--threads 1` (single-thread determinism,
    // e.g. golden reproduction) leave it at DuckDB's default so results stay
    // bit-exact. The spill directory is auto-created by the session when
    // `--temp-dir` is absent.
    let preserve_insertion_order = if source.threads == Some(1) {
        None
    } else {
        Some(false)
    };
    let mut session = Session::open_in_memory(SessionOptions {
        threads: source.threads,
        memory_limit: source.memory_limit.clone(),
        temp_directory: source.temp_dir.clone(),
        preserve_insertion_order,
        extensions: source.extensions.clone(),
        extension_directory: source.extension_dir.clone(),
        install_extensions: source.install_extensions,
        ..SessionOptions::default()
    })?;
    if let Some(catalog) = &source.catalog {
        let cfg = DuckLakeConfig {
            metadata: catalog.clone(),
            data_path: source.data_path.clone(),
            schema: source.schema.clone(),
            ..Default::default()
        };
        session
            .attach_ducklake(&cfg)
            .context("attaching DuckLake catalog")?;
    }
    Ok(session)
}

/// Register the configured input as a templated table.
fn register_input(session: &Session, source: &InputSource, templated: &str) -> Result<TableHandle> {
    if let Some(csv) = &source.input_csv {
        return Ok(session.register_csv(Path::new(csv), templated, &[])?);
    }
    if let Some(parquet) = &source.input_parquet {
        let p = parquet.replace('\'', "''");
        return Ok(session.register_table(&format!("read_parquet('{p}')"), templated)?);
    }
    if let Some(table) = &source.input {
        return register_lake(session, table, templated, source.as_of_version);
    }
    bail!("no input specified: use --input (lake), --input-csv, or --input-parquet");
}

/// Register a lake table, optionally as of a snapshot version (time travel).
fn register_lake(
    session: &Session,
    table: &str,
    templated: &str,
    as_of_version: Option<i64>,
) -> Result<TableHandle> {
    match as_of_version {
        Some(v) => Ok(session.register_lake_table_at_version(table, templated, v)?),
        None => Ok(session.register_lake_table(table, templated)?),
    }
}

/// Register a reference that is a lake table, a .parquet file, or a .csv file.
fn register_ref(
    session: &Session,
    reference: &str,
    templated: &str,
    as_of_version: Option<i64>,
) -> Result<TableHandle> {
    let ext = Path::new(reference)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    match ext.as_deref() {
        Some("parquet") => {
            let p = reference.replace('\'', "''");
            Ok(session.register_table(&format!("read_parquet('{p}')"), templated)?)
        }
        Some("csv") => Ok(session.register_csv(Path::new(reference), templated, &[])?),
        _ => register_lake(session, reference, templated, as_of_version),
    }
}

/// Resolve the input datasets. With `--dataset NAME=REF` entries, returns one
/// (handle, display_name) per dataset (link types); otherwise a single input
/// with an empty display name (dedupe).
fn resolve_inputs(session: &Session, source: &InputSource) -> Result<Vec<(TableHandle, String)>> {
    if source.datasets.is_empty() {
        return Ok(vec![(
            register_input(session, source, "input_table")?,
            String::new(),
        )]);
    }
    let mut out = Vec::new();
    for (i, spec) in source.datasets.iter().enumerate() {
        let (name, reference) = spec
            .split_once('=')
            .with_context(|| format!("--dataset must be NAME=REF, got `{spec}`"))?;
        let handle = register_ref(
            session,
            reference,
            &format!("input_dataset_{i}"),
            source.as_of_version,
        )?;
        out.push((handle, name.to_string()));
    }
    Ok(out)
}

fn build_linker<'a>(
    session: &'a Session,
    settings: Settings,
    inputs: &'a [(TableHandle, String)],
) -> Result<Linker<'a>> {
    let refs: Vec<(&TableHandle, &str)> = inputs.iter().map(|(h, n)| (h, n.as_str())).collect();
    Ok(Linker::new_multi(session, settings, &refs)?)
}

fn train(a: TrainArgs) -> Result<()> {
    if a.deterministic_rules.is_empty() {
        bail!("train requires at least one --deterministic-rule");
    }
    let settings =
        Settings::from_json_file(Path::new(&a.settings)).context("loading settings JSON")?;
    let session = open_session(&a.source)?;
    let inputs = resolve_inputs(&session, &a.source)?;

    let mut linker = build_linker(&session, settings, &inputs)?;
    let outcome = linker.train(&TrainConfig {
        deterministic_rules: a.deterministic_rules,
        recall: a.recall,
        u_max_pairs: a.max_pairs,
        em_rules: a.em_rules,
    })?;

    let model = linker.into_model();
    model.save_to_json_file(Path::new(&a.output_model))?;

    eprintln!(
        "trained: prior={:.6}, {} EM session(s)",
        outcome.prior,
        outcome.em_sessions.len()
    );
    for s in &outcome.em_sessions {
        eprintln!(
            "  EM blocked on `{}`: {} iterations{}, deactivated [{}]",
            s.blocking_rule,
            s.iterations.len() - 1,
            if s.converged { " (converged)" } else { "" },
            s.deactivated.join(", ")
        );
    }
    println!("{}", a.output_model);
    Ok(())
}

fn predict(a: PredictArgs) -> Result<()> {
    let settings = Settings::from_json_file(Path::new(&a.model)).context("loading model JSON")?;
    let session = open_session(&a.source)?;
    let inputs = resolve_inputs(&session, &a.source)?;

    let linker = build_linker(&session, settings, &inputs)?;

    // Streaming path: write each chunk straight to the lake table (constant
    // memory), never holding the full result in a temp table.
    if a.stream {
        if a.source.catalog.is_none() {
            bail!("--stream writes to the lake; provide --catalog");
        }
        if a.output_parquet.is_some() {
            bail!("--stream writes to the lake table; drop --output-parquet");
        }
        let n = linker.predict_chunked_to_lake(
            &a.output,
            a.threshold_match_probability,
            a.num_chunks,
            &a.source.lake_write_options(),
        )?;
        eprintln!(
            "streamed {n} predictions to lake table `{}` in {} pass(es)",
            a.output,
            a.num_chunks.max(1)
        );
        println!("{n}");
        return Ok(());
    }

    let predictions = linker.predict_chunked(a.threshold_match_probability, a.num_chunks)?;
    let n = session.count(&predictions.physical)?;

    if let Some(path) = &a.output_parquet {
        write_parquet(&session, &predictions, path)?;
    }
    if a.source.catalog.is_some() {
        session.write_to_lake_with(&predictions, &a.output, &a.source.lake_write_options())?;
        eprintln!("wrote {n} predictions to lake table `{}`", a.output);
    } else if a.output_parquet.is_none() {
        bail!("no output target: provide --catalog (lake) or --output-parquet");
    }
    println!("{n}");
    Ok(())
}

fn cluster(a: ClusterArgs) -> Result<()> {
    let settings = Settings::from_json_file(Path::new(&a.model)).context("loading model JSON")?;
    let session = open_session(&a.source)?;
    let inputs = resolve_inputs(&session, &a.source)?;

    let predictions = if let Some(parquet) = &a.predictions_parquet {
        let p = parquet.replace('\'', "''");
        session.register_table(&format!("read_parquet('{p}')"), "__dedupe__predict")?
    } else if let Some(table) = &a.predictions {
        session.register_lake_table(table, "__dedupe__predict")?
    } else {
        bail!("no predictions specified: use --predictions (lake) or --predictions-parquet");
    };

    let linker = build_linker(&session, settings, &inputs)?;
    let clusters = linker.cluster(&predictions, a.threshold)?;
    let n = session.count(&clusters.physical)?;

    if let Some(path) = &a.output_parquet {
        write_parquet(&session, &clusters, path)?;
    }
    if a.source.catalog.is_some() {
        session.write_to_lake_with(&clusters, &a.output, &a.source.lake_write_options())?;
        eprintln!("wrote {n} clustered rows to lake table `{}`", a.output);
    } else if a.output_parquet.is_none() {
        bail!("no output target: provide --catalog (lake) or --output-parquet");
    }
    println!("{n}");
    Ok(())
}

fn link_new(a: LinkNewArgs) -> Result<()> {
    let settings = Settings::from_json_file(Path::new(&a.model)).context("loading model JSON")?;
    let session = open_session(&a.source)?;

    // Existing records come from the standard --input/--input-csv/--input-parquet.
    let existing_input = register_input(&session, &a.source, "existing_input")?;
    // New batch comes from --new-input / --new-input-csv / --new-input-parquet.
    let new_input = register_new_input(&session, &a)?;

    let existing_concat = predict::materialize_concat(&session, &existing_input)?;
    let new_concat = predict::materialize_concat(&session, &new_input)?;
    // Term frequencies reflect the established (existing) data.
    let tf_tables = predict::materialize_tf_tables(&session, &settings, &existing_concat)?;

    let threshold_mw = a.threshold_match_probability.and_then(|p| {
        if p <= 0.0 {
            None
        } else {
            Some((p / (1.0 - p)).log2())
        }
    });

    let mut parts: Vec<TableHandle> = Vec::new();
    if matches!(a.scope, LinkNewScope::Between | LinkNewScope::Both) {
        parts.push(predict_between(
            &session,
            &settings,
            &existing_concat,
            &new_concat,
            &tf_tables,
            threshold_mw,
        )?);
    }
    if matches!(a.scope, LinkNewScope::Within | LinkNewScope::Both) {
        parts.push(predict_within(
            &session,
            &settings,
            &new_concat,
            &tf_tables,
            threshold_mw,
        )?);
    }

    let result = union_tables(&session, &parts)?;
    let n = session.count(&result.physical)?;

    if let Some(path) = &a.output_parquet {
        write_parquet(&session, &result, path)?;
    }
    if a.source.catalog.is_some() {
        session.write_to_lake_with(&result, &a.output, &a.source.lake_write_options())?;
        eprintln!(
            "wrote {n} incremental predictions to lake table `{}`",
            a.output
        );
    } else if a.output_parquet.is_none() {
        bail!("no output target: provide --catalog (lake) or --output-parquet");
    }
    println!("{n}");
    Ok(())
}

fn cluster_new(a: ClusterNewArgs) -> Result<()> {
    let settings = Settings::from_json_file(Path::new(&a.model)).context("loading model JSON")?;
    let session = open_session(&a.source)?;
    let input = register_input(&session, &a.source, "input_table")?;
    let all_concat = predict::materialize_concat(&session, &input)?;

    // Existing clusters, normalised to (node_id, cluster_id).
    let existing_raw = if let Some(parquet) = &a.existing_clusters_parquet {
        let p = parquet.replace('\'', "''");
        session.register_table(&format!("read_parquet('{p}')"), "existing_clusters_raw")?
    } else if let Some(table) = &a.existing_clusters {
        session.register_lake_table(table, "existing_clusters_raw")?
    } else {
        bail!("no existing clusters: use --existing-clusters or --existing-clusters-parquet");
    };
    let existing_clusters = session.register_table(
        &format!(
            "(select \"{uid}\" as node_id, cluster_id from {tbl})",
            uid = settings.unique_id_column,
            tbl = existing_raw.physical
        ),
        "existing_clusters",
    )?;

    let new_predictions = if let Some(parquet) = &a.predictions_parquet {
        let p = parquet.replace('\'', "''");
        session.register_table(&format!("read_parquet('{p}')"), "__dedupe__predict")?
    } else if let Some(table) = &a.predictions {
        session.register_lake_table(table, "__dedupe__predict")?
    } else {
        bail!("no new predictions: use --predictions or --predictions-parquet");
    };

    let clusters = incremental_cluster(
        &session,
        &settings,
        &all_concat,
        &existing_clusters,
        &new_predictions,
        ClusterOptions {
            threshold_match_probability: a.threshold,
        },
    )?;
    let joined = clustered_with_input_data(&session, &settings, &all_concat, &clusters)?;
    let n = session.count(&joined.physical)?;

    if let Some(path) = &a.output_parquet {
        write_parquet(&session, &joined, path)?;
    }
    if a.source.catalog.is_some() {
        session.write_to_lake_with(&joined, &a.output, &a.source.lake_write_options())?;
        eprintln!("wrote {n} clustered rows to lake table `{}`", a.output);
    } else if a.output_parquet.is_none() {
        bail!("no output target: provide --catalog (lake) or --output-parquet");
    }
    println!("{n}");
    Ok(())
}

/// Register the new-batch input for `link-new`.
fn register_new_input(session: &Session, a: &LinkNewArgs) -> Result<TableHandle> {
    if let Some(csv) = &a.new_input_csv {
        return Ok(session.register_csv(Path::new(csv), "new_input", &[])?);
    }
    if let Some(parquet) = &a.new_input_parquet {
        let p = parquet.replace('\'', "''");
        return Ok(session.register_table(&format!("read_parquet('{p}')"), "new_input")?);
    }
    if let Some(table) = &a.new_input {
        return Ok(session.register_lake_table(table, "new_input")?);
    }
    bail!(
        "no new batch specified: use --new-input (lake), --new-input-csv, or --new-input-parquet"
    );
}

/// UNION ALL a set of prediction tables into one temp table.
fn union_tables(session: &Session, parts: &[TableHandle]) -> Result<TableHandle> {
    if parts.len() == 1 {
        return Ok(parts[0].clone());
    }
    let physical = format!("__dedupe__incremental_{}", std::process::id());
    let union = parts
        .iter()
        .map(|t| format!("SELECT * FROM {}", t.physical))
        .collect::<Vec<_>>()
        .join("\nUNION ALL\n");
    session.execute_batch(&format!("CREATE TEMP TABLE {physical} AS {union}"))?;
    Ok(TableHandle {
        templated: "__dedupe__incremental".to_string(),
        physical,
        columns: parts[0].columns.clone(),
    })
}

fn count_comparisons(a: CountComparisonsArgs) -> Result<()> {
    if a.blocking_rules.is_empty() {
        bail!("count-comparisons requires at least one --blocking-rule");
    }
    let settings = Settings::from_json_file(Path::new(&a.settings)).context("loading settings")?;
    let session = open_session(&a.source)?;
    let inputs = resolve_inputs(&session, &a.source)?;
    let refs: Vec<(&TableHandle, &str)> = inputs.iter().map(|(h, n)| (h, n.as_str())).collect();
    let concat = predict::materialize_concat_multi(&session, &refs, &settings)?;

    let counts =
        count_comparisons_from_blocking_rules(&session, &settings, &concat, &a.blocking_rules)?;

    for c in &counts {
        eprintln!(
            "rule {}: marginal={} cumulative={} / {} possible  [{}]",
            c.match_key,
            c.marginal_comparison_count,
            c.cumulative_comparison_count,
            c.total_possible_comparison_count,
            c.blocking_rule,
        );
    }
    // Machine-readable summary on stdout.
    println!("{}", serde_json::to_string(&counts_to_json(&counts))?);
    Ok(())
}

fn counts_to_json(
    counts: &[dedupe_core::blocking_analysis::BlockingRuleCount],
) -> serde_json::Value {
    serde_json::Value::Array(
        counts
            .iter()
            .map(|c| {
                serde_json::json!({
                    "blocking_rule": c.blocking_rule,
                    "match_key": c.match_key,
                    "marginal_comparison_count": c.marginal_comparison_count,
                    "cumulative_comparison_count": c.cumulative_comparison_count,
                    "total_possible_comparison_count": c.total_possible_comparison_count,
                })
            })
            .collect(),
    )
}

fn accuracy(a: AccuracyArgs) -> Result<()> {
    let settings = Settings::from_json_file(Path::new(&a.model)).context("loading model JSON")?;
    let session = open_session(&a.source)?;
    let inputs = resolve_inputs(&session, &a.source)?;
    let linker = build_linker(&session, settings, &inputs)?;

    let truth_space = truth_space_from_labels_column(
        linker.session_ref(),
        linker.settings_ref(),
        linker.concat_ref(),
        linker.tf_tables_ref(),
        &AccuracyOptions {
            label_column: a.label_column,
            match_weight_round_to_nearest: Some(a.match_weight_round_to_nearest),
            ..Default::default()
        },
    )?;
    let n = session.count(&truth_space.physical)?;

    if let Some(path) = &a.output_parquet {
        write_parquet(&session, &truth_space, path)?;
    }
    if a.source.catalog.is_some() {
        session.write_to_lake_with(&truth_space, &a.output, &a.source.lake_write_options())?;
        eprintln!("wrote {n} truth-space rows to lake table `{}`", a.output);
    } else if a.output_parquet.is_none() {
        bail!("no output target: provide --catalog (lake) or --output-parquet");
    }
    println!("{n}");
    Ok(())
}

fn write_parquet(session: &Session, table: &TableHandle, path: &str) -> Result<()> {
    let p = path.replace('\'', "''");
    session.execute_batch(&format!(
        "COPY {} TO '{p}' (FORMAT PARQUET)",
        table.physical
    ))?;
    Ok(())
}
