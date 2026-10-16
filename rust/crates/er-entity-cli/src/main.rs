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
    /// Regenerate in memory and fail if any committed file is stale or missing.
    Check {
        /// The repository root (holds `entities/`). Defaults to the current dir.
        #[arg(long, default_value = ".")]
        root: PathBuf,
    },
}

fn load_tree(root: &Path) -> Result<GeneratedTree> {
    let entities_dir = root.join("entities");
    let catalog = Catalog::load_dir(&entities_dir)
        .with_context(|| format!("loading entities from {}", entities_dir.display()))?;
    let mut tree = generate(&catalog).context("generating artifacts")?;
    append_query_modules(&entities_dir, &catalog, &mut tree)?;
    Ok(tree)
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
    if stale.is_empty() {
        println!(
            "er-entity check: {} generated files are current",
            tree.files.len()
        );
        Ok(true)
    } else {
        eprintln!(
            "er-entity check: {} generated file(s) are stale or missing:",
            stale.len()
        );
        for path in &stale {
            eprintln!("  {path}");
        }
        eprintln!("run `er-entity gen` and commit the diff.");
        Ok(false)
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match &cli.command {
        Command::Gen { root } => run_gen(root).map(|()| true),
        Command::Check { root } => run_check(root),
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
