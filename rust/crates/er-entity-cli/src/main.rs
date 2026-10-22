//! `er-entity` — the entity-framework CLI.
//!
//! `gen` renders the committed artifact tree from `entities/`; `check` proves the
//! committed tree is current (the stale-output gate CI runs). Later phases add
//! `diff`, `migrate` and `verify`.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command as ProcCommand, ExitCode, Stdio};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use er_entity_gen::{GeneratedTree, generate};
use er_entity_spec::Catalog;

/// Run a `.rs` file's contents through `rustfmt` so the committed generated tree is
/// always `cargo fmt`-clean. Non-Rust files pass through unchanged; if `rustfmt` is
/// unavailable or errors, the unformatted source is used (and `check` stays
/// consistent because it formats the same way).
fn canonicalize(path: &Path, contents: &str) -> String {
    if path.extension().is_none_or(|e| e != "rs") {
        return contents.to_string();
    }
    let child = ProcCommand::new("rustfmt")
        .args(["--edition", "2024", "--emit", "stdout"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn();
    let Ok(mut child) = child else {
        return contents.to_string();
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(contents.as_bytes());
    }
    match child.wait_with_output() {
        Ok(out) if out.status.success() => {
            String::from_utf8(out.stdout).unwrap_or_else(|_| contents.to_string())
        }
        _ => contents.to_string(),
    }
}

#[derive(Parser)]
#[command(name = "er-entity", about = "Entity framework generator and checks")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Render the generated tree from `entities/` into the working tree.
    Gen {
        /// The repository root (holds `entities/`). Defaults to the current dir.
        #[arg(long, default_value = ".")]
        root: PathBuf,
    },
    /// Regenerate in memory and fail if any committed file is stale or missing,
    /// then plan every statement and run the E0xx passes.
    Check {
        /// The repository root (holds `entities/`). Defaults to the current dir.
        #[arg(long, default_value = ".")]
        root: PathBuf,
    },
    /// The one CI command: the stale-output check plus the analyser passes. Gains
    /// the oracle and the live-store diff in later phases.
    Verify {
        /// The repository root (holds `entities/`). Defaults to the current dir.
        #[arg(long, default_value = ".")]
        root: PathBuf,
    },
    /// Compare a live store's schema to the entity catalog; non-zero if they differ.
    Diff {
        /// The repository root (holds `entities/`). Defaults to the current dir.
        #[arg(long, default_value = ".")]
        root: PathBuf,
        /// The store to diff: `postgres` (the control database) or `duckdb` (one
        /// tenant's DuckLake, whose stage-materialised relations are skipped).
        #[arg(long, default_value = "postgres")]
        store: String,
        /// The Postgres DSN (defaults to the `ER_PG_DSN` env var).
        #[arg(long)]
        dsn: Option<String>,
        /// `--store duckdb`: the DuckLake metadata (everything after `ducklake:`,
        /// e.g. a catalog file path or `postgres:dbname=...`); defaults to
        /// `ER_LAKE_CATALOG`.
        #[arg(long)]
        lake_catalog: Option<String>,
        /// `--store duckdb`: the lake's data path; defaults to `ER_LAKE_DATA_PATH`.
        #[arg(long)]
        lake_data_path: Option<String>,
    },
    /// Print the entity-relationship diagram (`docs/erd.md`) to stdout. `gen`
    /// writes the same document into the tree; `verify` fails when it is stale.
    Erd {
        /// The repository root (holds `entities/`). Defaults to the current dir.
        #[arg(long, default_value = ".")]
        root: PathBuf,
        /// Write to this path instead of stdout.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Run the entity-convention rules (the V-rules) on their own, or ask the
    /// registry about itself. `verify` runs the same pass first.
    Lint {
        /// The repository root (holds `entities/`). Defaults to the current dir.
        #[arg(long, default_value = ".")]
        root: PathBuf,
        /// List every registered rule instead of checking the tree.
        #[arg(long, conflicts_with_all = ["explain", "docs"])]
        list: bool,
        /// Explain one rule by code (`--explain V4`), case-insensitive.
        #[arg(long, value_name = "CODE", conflicts_with = "docs")]
        explain: Option<String>,
        /// Print the generated rule reference (`docs/entity-lint-rules.md`),
        /// which `gen` writes and `verify` byte-compares.
        #[arg(long)]
        docs: bool,
    },
    /// Plan (or `--apply`) the safe migration that reconciles a live store to the
    /// catalog; a breaking difference is refused for a hand-written migration (the
    /// control database) or a rebuild (a lake). `--apply` is the same reconciler
    /// er-server runs at boot and at every lake job, ledger included.
    Migrate {
        /// The repository root (holds `entities/`). Defaults to the current dir.
        #[arg(long, default_value = ".")]
        root: PathBuf,
        /// The store: `postgres` (the control database) or `duckdb` (one tenant's
        /// DuckLake).
        #[arg(long, default_value = "postgres")]
        store: String,
        /// The Postgres DSN (defaults to the `ER_PG_DSN` env var).
        #[arg(long)]
        dsn: Option<String>,
        /// `--store duckdb`: the DuckLake metadata; defaults to `ER_LAKE_CATALOG`.
        #[arg(long)]
        lake_catalog: Option<String>,
        /// `--store duckdb`: the lake's data path; defaults to `ER_LAKE_DATA_PATH`.
        #[arg(long)]
        lake_data_path: Option<String>,
        /// Apply the safe statements instead of only printing them.
        #[arg(long)]
        apply: bool,
    },
}

/// Load the catalog and hold it to the entity conventions.
///
/// Every path that reads `entities/` goes through here rather than
/// [`Catalog::load_dir`] directly, because `load_dir` checks only what makes a
/// catalog structurally coherent — the conventions the generator relies on (an
/// `id` to key row images by, the universal timestamps) are `er-entity-lint`'s.
/// Warnings are not reported here: that would print the same list on every `gen`,
/// `check`, `diff` and `migrate`. `er-entity lint` is where they surface.
fn load_catalog(entities_dir: &Path) -> Result<Catalog> {
    match er_entity_lint::load_linted(entities_dir) {
        Ok(catalog) => Ok(catalog),
        Err(er_entity_lint::LintError::Rules { diagnostics }) => {
            eprintln!("er-entity: {} entity lint error(s):", diagnostics.len());
            for d in &diagnostics {
                eprintln!("  [{}] {}: {}", d.code, d.path.display(), d.message);
            }
            anyhow::bail!(
                "the entity conventions are not met — `er-entity lint --explain <code>` \
                 says why each rule exists"
            )
        }
        Err(err) => {
            Err(err).with_context(|| format!("loading entities from {}", entities_dir.display()))
        }
    }
}

fn load_tree(root: &Path) -> Result<GeneratedTree> {
    let entities_dir = root.join("entities");
    let catalog = load_catalog(&entities_dir)?;
    let mut tree = generate(&catalog).context("generating artifacts")?;
    append_query_modules(&entities_dir, &catalog, &mut tree)?;
    tree.files.push(er_entity_gen::GeneratedFile {
        path: PathBuf::from("docs/erd.md"),
        contents: render_erd(root, &catalog)?,
    });
    // The rule reference is generated from the registry and byte-compared by
    // `check`, so the documented rule set cannot drift from the enforced one —
    // which is how three E-codes came to be documented but never implemented.
    tree.files.push(er_entity_gen::GeneratedFile {
        path: PathBuf::from("docs/entity-lint-rules.md"),
        contents: er_entity_lint::rules_markdown(),
    });
    Ok(tree)
}

/// Render `docs/erd.md` from the catalog, the er-schema registry and (when the
/// frontend tree is present) the latest drizzle snapshot of the `erweb` schema.
fn render_erd(root: &Path, catalog: &Catalog) -> Result<String> {
    let snapshot = load_erweb_snapshot(root)?;
    er_entity_gen::emit_erd(catalog, snapshot.as_deref()).context("rendering the ERD")
}

/// The latest `frontend/drizzle/meta/<NNNN>_snapshot.json`, or `None` when the
/// frontend tree is absent (the snapshot indices sort lexicographically).
fn load_erweb_snapshot(root: &Path) -> Result<Option<String>> {
    let meta = root.join("frontend/drizzle/meta");
    if !meta.is_dir() {
        return Ok(None);
    }
    let mut snapshots: Vec<PathBuf> = fs::read_dir(&meta)
        .with_context(|| format!("reading {}", meta.display()))?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with("_snapshot.json"))
        })
        .collect();
    snapshots.sort();
    match snapshots.last() {
        Some(path) => {
            let text =
                fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
            Ok(Some(text))
        }
        None => Ok(None),
    }
}

fn run_erd(root: &Path, out: Option<&Path>) -> Result<()> {
    let entities_dir = root.join("entities");
    let catalog = load_catalog(&entities_dir)?;
    let erd = render_erd(root, &catalog)?;
    match out {
        Some(path) => {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("creating {}", parent.display()))?;
            }
            fs::write(path, &erd).with_context(|| format!("writing {}", path.display()))?;
            println!("wrote {}", path.display());
        }
        None => print!("{erd}"),
    }
    Ok(())
}

/// Append the generated typed query functions (from the analyser) to each entity's
/// Rust module. The struct + arrow_schema() come from er-entity-gen; the query
/// functions come from er-sql-check (a text transform of the validated SQL). The
/// result is one `src/<entity>.rs` carrying the struct and its `read`/`create`/…
/// query modules, so a call site reads as `golden_record::read::list_page`.
fn append_query_modules(
    entities_dir: &Path,
    catalog: &er_entity_spec::Catalog,
    tree: &mut GeneratedTree,
) -> Result<()> {
    let statements = er_sql_check::load_statements(entities_dir, catalog)
        .context("loading statements for the query emitter")?;
    for entity in catalog.entities() {
        let Some(module) = er_sql_check::emit_query_modules(entity, &statements) else {
            continue;
        };
        let rel = format!("rust/crates/er-entities/src/{}.rs", entity.name);
        let file = tree
            .files
            .iter_mut()
            .find(|f| f.path == Path::new(&rel))
            .with_context(|| format!("no generated Rust module for entity {}", entity.name))?;
        file.contents.push_str(&module);
    }
    Ok(())
}

fn run_gen(root: &Path) -> Result<()> {
    let tree = load_tree(root)?;
    for file in &tree.files {
        let dest = root.join(&file.path);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        let contents = canonicalize(&file.path, &file.contents);
        fs::write(&dest, &contents).with_context(|| format!("writing {}", dest.display()))?;
        println!("wrote {}", file.path.display());
    }
    Ok(())
}

fn run_check(root: &Path) -> Result<bool> {
    let tree = load_tree(root)?;
    let mut stale: Vec<String> = Vec::new();
    for file in &tree.files {
        let dest = root.join(&file.path);
        let current = fs::read_to_string(&dest).ok();
        let expected = canonicalize(&file.path, &file.contents);
        if current.as_deref() != Some(expected.as_str()) {
            stale.push(file.path.display().to_string());
        }
    }
    let orphans = orphaned_outputs(root, &tree)?;
    if stale.is_empty() && orphans.is_empty() {
        println!(
            "er-entity check: {} generated files are current",
            tree.files.len()
        );
        Ok(true)
    } else {
        if !stale.is_empty() {
            eprintln!(
                "er-entity check: {} generated file(s) are stale or missing:",
                stale.len()
            );
            for path in &stale {
                eprintln!("  {path}");
            }
            eprintln!("run `er-entity gen` and commit the diff.");
        }
        if !orphans.is_empty() {
            eprintln!(
                "er-entity check: {} orphaned file(s) in generated directories \
                 (left behind by a deleted entity/statement — remove them):",
                orphans.len()
            );
            for path in &orphans {
                eprintln!("  {path}");
            }
        }
        Ok(false)
    }
}

/// Files living in the generated-only destination directories that this gen run
/// did not emit: stale leftovers from a deleted entity or statement, which
/// `gen` never removes on its own.
fn orphaned_outputs(root: &Path, tree: &er_entity_gen::GeneratedTree) -> Result<Vec<String>> {
    let generated: std::collections::BTreeSet<PathBuf> =
        tree.files.iter().map(|f| f.path.clone()).collect();
    let mut orphans = Vec::new();
    for dir in er_entity_gen::GENERATED_DIRS {
        let base = root.join(dir);
        if !base.is_dir() {
            continue;
        }
        for entry in fs::read_dir(&base).with_context(|| format!("reading {}", base.display()))? {
            let path = entry?.path();
            if !path.is_file() {
                continue;
            }
            let rel = PathBuf::from(dir).join(path.file_name().unwrap_or_default());
            if !generated.contains(&rel) {
                orphans.push(rel.display().to_string());
            }
        }
    }
    orphans.sort();
    Ok(orphans)
}

/// Run the entity-convention rules and report what they find.
///
/// Errors fail the gate; warnings are printed and do not. Returns true when no
/// error-severity rule fired.
fn run_lint(root: &Path) -> Result<bool> {
    let entities_dir = root.join("entities");
    let catalog = Catalog::load_dir(&entities_dir)
        .with_context(|| format!("loading entities from {}", entities_dir.display()))?;
    let diags = er_entity_lint::lint_catalog(&catalog, &entities_dir);
    let (errors, warnings): (Vec<_>, Vec<_>) = diags
        .into_iter()
        .partition(|d| d.severity == er_entity_lint::Severity::Error);

    for d in &warnings {
        eprintln!(
            "  [{}] warning: {}: {}",
            d.code,
            d.path.display(),
            d.message
        );
    }
    if !warnings.is_empty() {
        eprintln!("er-entity lint: {} warning(s)", warnings.len());
    }

    if errors.is_empty() {
        // Say "no errors", not "holds every rule": warnings are real findings,
        // and a summary that ignored them would read as a clean bill of health.
        println!(
            "er-entity lint: {} rules over {} entities, no errors",
            er_entity_lint::rules().len(),
            catalog.entities().len()
        );
        Ok(true)
    } else {
        eprintln!("er-entity lint: {} error(s):", errors.len());
        for d in &errors {
            eprintln!("  [{}] {}: {}", d.code, d.path.display(), d.message);
        }
        eprintln!("`er-entity lint --explain <code>` says why each rule exists.");
        Ok(false)
    }
}

/// `--list`: the registry as a table.
fn print_rule_list() {
    let rules = er_entity_lint::rules();
    println!("{:<6} {:<9} INVARIANT", "CODE", "SEVERITY");
    for info in &rules {
        println!("{:<6} {:<9} {}", info.code.0, info.severity, info.title);
    }
    println!(
        "\n{} rules. `--explain <code>` for the reasoning.",
        rules.len()
    );
}

/// `--explain <code>`: one rule in full.
fn print_rule_explanation(code: &str) -> Result<()> {
    let Some(info) = er_entity_lint::explain(code) else {
        anyhow::bail!("no rule {code:?} — `er-entity lint --list` has the registry")
    };
    println!("{} ({})\n", info.code, info.severity);
    println!("{}\n", info.title);
    println!("{}\n", info.explain);
    println!("Conforming:\n\n{}", info.conforming.trim_end());
    println!("\nViolating:\n\n{}", info.violating.trim_end());
    Ok(())
}

/// Plan every statement and run the E0xx passes against the two-store catalog.
fn run_analyze(root: &Path) -> Result<bool> {
    let entities_dir = root.join("entities");
    let catalog = load_catalog(&entities_dir)?;
    let diags =
        er_sql_check::analyze_catalog(&entities_dir, &catalog).context("analysing statements")?;
    if diags.is_empty() {
        println!("er-entity analyse: every statement plans and passes the E0xx rules");
        Ok(true)
    } else {
        eprintln!("er-entity analyse: {} diagnostic(s):", diags.len());
        for d in &diags {
            eprintln!("  [{}] {}: {}", d.code, d.path.display(), d.message);
        }
        Ok(false)
    }
}

/// The entity rules, the stale-output check and the analyser passes (the
/// `check`/`verify` body).
///
/// The lint pass runs first and short-circuits, because every later pass needs a
/// conforming catalog to say anything meaningful: the generator keys row images
/// by `id` (V1) and the analyser's E005/E014 rules assume the universal
/// timestamps (V4). Reporting stale artifacts generated from a catalog that
/// breaks those would be noise on top of the real cause. Check and analyse still
/// both run, as they always have, so a single pass shows everything.
fn run_verify(root: &Path) -> Result<bool> {
    if !run_lint(root)? {
        return Ok(false);
    }
    let fresh = run_check(root)?;
    let analysed = run_analyze(root)?;
    Ok(fresh && analysed)
}

/// How a lake is reached from the command line: explicit flags, else the env the
/// org row carries (`ER_LAKE_CATALOG` / `ER_LAKE_DATA_PATH`).
struct LakeTarget {
    catalog: String,
    data_path: Option<String>,
}

impl LakeTarget {
    fn resolve(catalog: Option<&str>, data_path: Option<&str>) -> Result<Self> {
        let catalog = catalog
            .map(ToString::to_string)
            .or_else(|| std::env::var("ER_LAKE_CATALOG").ok())
            .context("a lake is required (--lake-catalog or ER_LAKE_CATALOG)")?;
        let data_path = data_path
            .map(ToString::to_string)
            .or_else(|| std::env::var("ER_LAKE_DATA_PATH").ok());
        Ok(Self { catalog, data_path })
    }

    fn open(&self, read_only: bool) -> Result<duckdb::Connection> {
        let conn = duckdb::Connection::open_in_memory().context("opening DuckDB")?;
        er_entity_diff::attach_lake(&conn, &self.catalog, self.data_path.as_deref(), read_only)
            .with_context(|| format!("attaching the lake {}", self.catalog))?;
        Ok(conn)
    }
}

fn parse_store(store: &str) -> Result<er_entity_spec::Store> {
    use er_entity_spec::Store;
    match store {
        "postgres" => Ok(Store::Postgres),
        "duckdb" => Ok(Store::Duckdb),
        other => anyhow::bail!("--store must be `postgres` or `duckdb` (got {other:?})"),
    }
}

fn print_diffs(diffs: &[er_entity_diff::DiffItem], store: er_entity_spec::Store) -> bool {
    use er_entity_diff::Severity;
    let label = match store {
        er_entity_spec::Store::Postgres => "Postgres",
        er_entity_spec::Store::Duckdb => "lake",
    };
    let pending = diffs
        .iter()
        .filter(|d| d.severity != Severity::Ahead)
        .count();
    if pending == 0 {
        println!("er-entity diff: the {label} schema matches the catalog");
    } else {
        eprintln!("er-entity diff: {pending} difference(s):");
    }
    for d in diffs {
        match d.severity {
            Severity::Ahead => eprintln!("  [Ahead] {}: {}", d.relation, d.message),
            severity => eprintln!("  [{severity:?}] {}: {}", d.relation, d.message),
        }
    }
    pending == 0
}

/// Diff a live store against the catalog; returns true when they match (an
/// ahead store — extra live columns — still matches; it is printed).
fn run_diff(
    root: &Path,
    store: &str,
    dsn: Option<&str>,
    lake_catalog: Option<&str>,
    lake_data_path: Option<&str>,
) -> Result<bool> {
    use er_entity_spec::Store;
    let store = parse_store(store)?;
    let entities_dir = root.join("entities");
    let catalog = load_catalog(&entities_dir)?;

    let diffs = match store {
        Store::Duckdb => {
            let target = LakeTarget::resolve(lake_catalog, lake_data_path)?;
            let conn = target.open(true)?;
            let stage: std::collections::BTreeSet<String> =
                er_schema::model::stage_owned().into_iter().collect();
            let owned = er_entity_diff::catalog_without(&catalog, Store::Duckdb, &stage)?;
            let relations = er_entity_diff::lake_relations(&catalog);
            let live = er_entity_diff::read_lake_columns(&conn, &relations)
                .context("reading the live lake schema")?;
            er_entity_diff::diff_store(&owned, Store::Duckdb, &live)
        }
        Store::Postgres => {
            let dsn = dsn
                .map(ToString::to_string)
                .or_else(|| std::env::var("ER_PG_DSN").ok())
                .context("a Postgres DSN is required (--dsn or ER_PG_DSN)")?;
            let relations: Vec<String> = er_entity_diff::postgres_relations(&catalog);
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .context("building the tokio runtime")?;
            runtime.block_on(async {
                let (client, connection) = tokio_postgres::connect(&dsn, er_pg_tls::make_tls())
                    .await
                    .context("connecting to Postgres")?;
                let handle = tokio::spawn(connection);
                let live = er_entity_diff::read_postgres_columns(&client, &relations)
                    .await
                    .context("reading the live schema")?;
                handle.abort();
                anyhow::Ok(er_entity_diff::diff_store(&catalog, Store::Postgres, &live))
            })?
        }
    };
    Ok(print_diffs(&diffs, store))
}

fn print_plan(plan: &er_entity_diff::MigrationPlan, label: &str) -> bool {
    if !plan.breaking.is_empty() {
        eprintln!(
            "ERR_SCHEMA_BREAKING: {} change(s) the reconciler refuses to auto-apply:",
            plan.breaking.len()
        );
        for d in &plan.breaking {
            eprintln!("  {}: {}", d.relation, d.message);
        }
        return false;
    }
    for d in &plan.ahead {
        eprintln!("  [Ahead] {}: {}", d.relation, d.message);
    }
    if plan.statements.is_empty() {
        println!("er-entity migrate: the {label} schema is already current");
    } else {
        println!(
            "-- er-entity migrate plan ({} statement(s)):",
            plan.statements.len()
        );
        for stmt in &plan.statements {
            println!("{stmt}");
        }
    }
    true
}

fn report_reconciled(reconciled: &er_entity_diff::Reconciled, label: &str) {
    for d in &reconciled.ahead {
        eprintln!("  [Ahead] {}: {}", d.relation, d.message);
    }
    if reconciled.applied.is_empty() {
        println!(
            "er-entity migrate: the {label} schema is already current (catalog {})",
            &reconciled.catalog_hash[..12]
        );
    } else {
        println!(
            "er-entity migrate: applied {} statement(s) to the {label} (catalog {})",
            reconciled.applied.len(),
            &reconciled.catalog_hash[..12]
        );
        for stmt in &reconciled.applied {
            println!("{stmt}");
        }
    }
}

/// Plan — or, with `apply`, apply — the safe migration reconciling a store to the
/// catalog. A breaking difference is refused (ERR_SCHEMA_BREAKING). `apply` runs
/// the same reconciler er-server does, so the store's `schema_migrations` ledger
/// records what this command did.
fn run_migrate(
    root: &Path,
    store: &str,
    dsn: Option<&str>,
    lake_catalog: Option<&str>,
    lake_data_path: Option<&str>,
    apply: bool,
) -> Result<bool> {
    use er_entity_spec::Store;
    let store = parse_store(store)?;
    let entities_dir = root.join("entities");
    let catalog = load_catalog(&entities_dir)?;
    let applied_by = format!("er-entity {}", env!("CARGO_PKG_VERSION"));

    match store {
        Store::Duckdb => {
            let target = LakeTarget::resolve(lake_catalog, lake_data_path)?;
            let conn = target.open(!apply)?;
            if apply {
                match er_entity_diff::reconcile_lake(&conn, &catalog, &applied_by) {
                    Ok(reconciled) => {
                        report_reconciled(&reconciled, "lake");
                        Ok(true)
                    }
                    Err(e @ er_entity_diff::ReconcileError::Breaking(_)) => {
                        eprintln!("{e}");
                        Ok(false)
                    }
                    Err(e) => Err(e).context("reconciling the lake"),
                }
            } else {
                let stage: std::collections::BTreeSet<String> =
                    er_schema::model::stage_owned().into_iter().collect();
                let owned = er_entity_diff::catalog_without(&catalog, Store::Duckdb, &stage)?;
                let relations = er_entity_diff::lake_relations(&catalog);
                let live = er_entity_diff::read_lake_columns(&conn, &relations)
                    .context("reading the live lake schema")?;
                let plan = er_entity_diff::plan_migration(&owned, Store::Duckdb, &live);
                Ok(print_plan(&plan, "lake"))
            }
        }
        Store::Postgres => {
            let dsn = dsn
                .map(ToString::to_string)
                .or_else(|| std::env::var("ER_PG_DSN").ok())
                .context("a Postgres DSN is required (--dsn or ER_PG_DSN)")?;
            let relations: Vec<String> = er_entity_diff::postgres_relations(&catalog);
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .context("building the tokio runtime")?;
            runtime.block_on(async {
                let (mut client, connection) = tokio_postgres::connect(&dsn, er_pg_tls::make_tls())
                    .await
                    .context("connecting to Postgres")?;
                let handle = tokio::spawn(connection);
                let outcome = if apply {
                    match er_entity_diff::reconcile_postgres(&mut client, &catalog, &applied_by)
                        .await
                    {
                        Ok(reconciled) => {
                            report_reconciled(&reconciled, "Postgres");
                            Ok(true)
                        }
                        Err(e @ er_entity_diff::ReconcileError::Breaking(_)) => {
                            eprintln!("{e}");
                            Ok(false)
                        }
                        Err(e) => Err(e).context("reconciling the control database"),
                    }
                } else {
                    let live = er_entity_diff::read_postgres_columns(&client, &relations)
                        .await
                        .context("reading the live schema")?;
                    let plan = er_entity_diff::plan_migration(&catalog, Store::Postgres, &live);
                    Ok(print_plan(&plan, "Postgres"))
                };
                handle.abort();
                outcome
            })
        }
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match &cli.command {
        Command::Gen { root } => run_gen(root).map(|()| true),
        Command::Check { root } | Command::Verify { root } => run_verify(root),
        Command::Erd { root, out } => run_erd(root, out.as_deref()).map(|()| true),
        Command::Lint {
            root,
            list,
            explain,
            docs,
        } => {
            if *list {
                print_rule_list();
                Ok(true)
            } else if let Some(code) = explain {
                print_rule_explanation(code).map(|()| true)
            } else if *docs {
                print!("{}", er_entity_lint::rules_markdown());
                Ok(true)
            } else {
                run_lint(root)
            }
        }
        Command::Diff {
            root,
            store,
            dsn,
            lake_catalog,
            lake_data_path,
        } => run_diff(
            root,
            store,
            dsn.as_deref(),
            lake_catalog.as_deref(),
            lake_data_path.as_deref(),
        ),
        Command::Migrate {
            root,
            store,
            dsn,
            lake_catalog,
            lake_data_path,
            apply,
        } => run_migrate(
            root,
            store,
            dsn.as_deref(),
            lake_catalog.as_deref(),
            lake_data_path.as_deref(),
            *apply,
        ),
    };
    match result {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(err) => {
            eprintln!("er-entity: {err:#}");
            ExitCode::FAILURE
        }
    }
}
