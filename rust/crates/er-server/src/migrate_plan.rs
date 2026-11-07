//! What the next release would do to the control-plane schema, without doing it.
//!
//! `er-server migrate` already refuses a breaking catalog — but only from
//! phase 3, and [`crate::pg::ensure_schema`] is six transactions, not one. By
//! the time the reconciler says no, the baseline replay and the
//! `BeforeReconcile` fixups have **committed**. That is correct for a boot (it
//! converges as far as it safely can and then halts loudly) and useless for a
//! deploy gate, which has to answer "is this release additive?" over a store it
//! has not touched.
//!
//! So this plans and writes nothing. It is the gate
//! the `entity-framework` skill's `references/entity-framework-design.md` names
//! as the next step in its own rule 9:
//!
//! > A rollback across a breaking change is not protected by machinery yet: the
//! > image carries no `additive`/`breaking` label and CD does not refuse it,
//! > because there is no production environment to protect and no CD pipeline
//! > to put the gate in. That label is the next step when one exists.
//!
//! The classification is not re-derived here. [`er_entity_diff::plan_migration`]
//! is a pure function over (catalog, live columns), already the authority for
//! what the reconciler will and will not apply, and already tested against a
//! real Postgres. This module reads the live columns, calls it, and renders the
//! answer.
//!
//! **Why this cannot be `er-entity migrate`**, which does the same thing: that
//! binary loads the catalog from `entities/` on disk, and the deployed image
//! carries only the `er-server` multicall binary (`rust/Dockerfile`). A gate
//! that needs the repository checked out cannot run in the pod being deployed.
//! This reads the catalog compiled into the image, which is also the only
//! catalog whose answer is relevant.
//!
//! ## What the plan does not cover, stated because it is a real limit
//!
//! The named fixups (`crate::fixups`). They exist precisely to perform what the
//! declarative reconciler refuses — `0003_per_dataset_locks` drops indexes and a
//! constraint — so the planner cannot see them and a verdict of `additive` is a
//! statement about the DIFF, not about everything the boot will execute. That is
//! the intended division: a fixup is a named, reviewed, ledgered step whose
//! safety is argued in its own doc comment, while the diff is the part generated
//! from `entities/` that nobody reads line by line.

use er_entity_diff::{MigrationPlan, Severity};
use er_entity_spec::Store;
use serde_json::{Value, json};

/// The three answers, in increasing severity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// The live store already matches the catalog. Nothing to apply.
    Current,
    /// Everything the catalog asks for can be auto-applied: `CREATE TABLE` for
    /// an absent relation, `ADD COLUMN` for an absent nullable one. Both the
    /// old and the new binary tolerate either state, which is what makes a
    /// rollout that overlaps them safe.
    Additive,
    /// At least one difference the reconciler refuses. A deploy carrying this
    /// cannot be rolled out underneath a running older version.
    Breaking,
}

impl Verdict {
    /// The wire/CLI spelling.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::Additive => "additive",
            Self::Breaking => "breaking",
        }
    }

    /// The process exit code.
    ///
    /// `0` for both safe answers, because a caller asking "may I deploy this?"
    /// wants one check and not two — `current` and `additive` are the same
    /// answer to that question. `3` rather than `1` for breaking, so a gate can
    /// tell a refusal apart from a failure to reach the database at all; `1`
    /// stays what every other error path in the binary already means.
    #[must_use]
    pub fn exit_code(self) -> u8 {
        match self {
            Self::Current | Self::Additive => 0,
            Self::Breaking => 3,
        }
    }
}

/// Read a plan's verdict.
///
/// `breaking` dominates: a plan with both safe statements and a refused
/// difference is breaking, because applying the safe half would leave the store
/// between two shapes.
#[must_use]
pub fn verdict(plan: &MigrationPlan) -> Verdict {
    if !plan.breaking.is_empty() {
        Verdict::Breaking
    } else if plan.statements.is_empty() {
        Verdict::Current
    } else {
        Verdict::Additive
    }
}

/// The machine-readable form, for a deploy gate to branch on.
///
/// `ahead` is carried even though it never changes the verdict, because it is
/// the signal that something is serving this store from a LATER catalog than
/// the one being planned — an image rolled back across an additive release. An
/// operator seeing `additive` plus a non-empty `ahead` is looking at a
/// rollback, not a fresh deploy, and the two want different attention.
#[must_use]
pub fn as_json(plan: &MigrationPlan, catalog_hash: &str) -> Value {
    json!({
        "verdict": verdict(plan).as_str(),
        "catalog_hash": catalog_hash,
        "statements": plan.statements.iter().map(|s| json!({
            "relation": s.relation,
            "sql": s.sql,
        })).collect::<Vec<_>>(),
        "breaking": plan.breaking.iter().map(render_diff).collect::<Vec<_>>(),
        "ahead": plan.ahead.iter().map(render_diff).collect::<Vec<_>>(),
    })
}

fn render_diff(item: &er_entity_diff::DiffItem) -> Value {
    json!({
        "relation": item.relation,
        "severity": match item.severity {
            Severity::Safe => "safe",
            Severity::Breaking => "breaking",
            Severity::Ahead => "ahead",
        },
        "message": item.message,
    })
}

/// The human-readable form.
///
/// Printed to stdout rather than stderr: this is the command's OUTPUT, and a
/// caller piping it to a file should get the report, not an empty file and a
/// log.
#[must_use]
pub fn render(plan: &MigrationPlan, catalog_hash: &str) -> String {
    use std::fmt::Write as _;

    let mut out = String::new();
    let verdict = verdict(plan);
    // Writing to a String is infallible, so the Results are discarded rather
    // than unwrapped — an `expect` here would be a panic path in the gate whose
    // whole job is to refuse a bad deploy cleanly.
    let _ = writeln!(
        out,
        "verdict: {} (catalog {})",
        verdict.as_str(),
        &catalog_hash[..catalog_hash.len().min(12)]
    );

    if !plan.statements.is_empty() {
        let _ = writeln!(
            out,
            "\n{} statement(s) would be applied:",
            plan.statements.len()
        );
        for statement in &plan.statements {
            let _ = writeln!(
                out,
                "  {}: {}",
                statement.relation,
                one_line(&statement.sql)
            );
        }
    }
    if !plan.ahead.is_empty() {
        let _ = writeln!(
            out,
            "\n{} column(s) AHEAD of this catalog (tolerated, never dropped — a \
             later image has served this store):",
            plan.ahead.len()
        );
        for item in &plan.ahead {
            let _ = writeln!(out, "  {}: {}", item.relation, item.message);
        }
    }
    if !plan.breaking.is_empty() {
        let _ = writeln!(
            out,
            "\n{} difference(s) the reconciler REFUSES to auto-apply:",
            plan.breaking.len()
        );
        for item in &plan.breaking {
            let _ = writeln!(out, "  {}: {}", item.relation, item.message);
        }
        out.push_str(
            "\nThis release cannot be rolled out underneath a running older version.\n\
             Removing or retyping a column takes TWO releases: N stops reading it,\n\
             N+1 changes it once no older pod is left. Where that is genuinely not\n\
             possible, the operation belongs in a named fixup (er-server/src/fixups.rs)\n\
             whose doc comment argues why it is safe.\n",
        );
    }
    out
}

/// How much of a statement the human report shows.
///
/// Collapsing whitespace is not enough on its own: the history prelude is a
/// whole plpgsql function with its own comments, and on an empty store it
/// renders as a single ~1500-character line that pushes the other twenty-one
/// statements off the screen. The point of the report is to see WHAT would be
/// applied and where; the full SQL is in `--json` for anything that needs it.
const SQL_PREVIEW: usize = 120;

/// Collapse DDL to one short line so a report stays scannable.
fn one_line(sql: &str) -> String {
    let flat = sql.split_whitespace().collect::<Vec<_>>().join(" ");
    // `char_indices` rather than a byte slice: the generated DDL carries
    // comments with non-ASCII punctuation, and `&flat[..n]` would panic
    // mid-character. A panic in the gate that refuses a bad deploy is the
    // worst place for one.
    match flat.char_indices().nth(SQL_PREVIEW) {
        None => flat,
        Some((at, _)) => format!("{}… ({} chars)", &flat[..at], flat.chars().count()),
    }
}

/// The catalog hash for the control store, as the ledger records it.
#[must_use]
pub fn control_catalog_hash() -> String {
    er_entity_diff::catalog_hash(&er_entities::catalog::catalog(), Store::Postgres)
}

/// What `er-server migrate` was asked to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MigrateArgs {
    /// Report and write nothing, whatever the verdict.
    pub plan_only: bool,
    /// Plan first and refuse — writing nothing — if the plan is breaking;
    /// otherwise apply.
    pub expect_additive: bool,
    /// Machine-readable output.
    pub json: bool,
}

impl MigrateArgs {
    /// Whether this invocation needs a plan at all.
    #[must_use]
    pub fn plans(self) -> bool {
        self.plan_only || self.expect_additive
    }
}

/// Parse `er-server migrate`'s arguments, refusing anything unrecognised.
///
/// Extracted from the binary so the refusal is testable, and it is testable
/// because the refusal is the whole point. The chart invokes this command from
/// a Helm hook, where the arguments are a string in a YAML file: a misspelled
/// `--expect-aditive` that were merely IGNORED would silently turn the gate
/// back into a plain apply, and the deploy would look identical while having
/// lost the thing it was added for.
///
/// That is not hypothetical. Probing the hook against a cluster running an
/// image built before this flag existed, the old binary — which took no
/// arguments at all — accepted `--does-not-exist` and exited 0 having done a
/// normal migrate. Silent acceptance is the behaviour being removed.
///
/// # Errors
/// The first unrecognised argument, named.
pub fn parse_migrate_args(args: &[String]) -> Result<MigrateArgs, String> {
    const KNOWN: [&str; 3] = ["--plan", "--json", "--expect-additive"];
    if let Some(unknown) = args.iter().find(|a| !KNOWN.contains(&a.as_str())) {
        return Err(format!(
            "er-server migrate: unknown argument {unknown:?} (expected {})",
            KNOWN.join(", ")
        ));
    }
    let has = |name: &str| args.iter().any(|a| a == name);
    Ok(MigrateArgs {
        plan_only: has("--plan"),
        expect_additive: has("--expect-additive"),
        json: has("--json"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use er_entity_diff::{DiffItem, Planned};

    fn plan(
        statements: Vec<Planned>,
        breaking: Vec<DiffItem>,
        ahead: Vec<DiffItem>,
    ) -> MigrationPlan {
        MigrationPlan {
            statements,
            breaking,
            ahead,
        }
    }

    fn diff(severity: Severity) -> DiffItem {
        DiffItem {
            relation: "public.jobs".into(),
            severity,
            message: "column x: integer -> bigint".into(),
        }
    }

    fn statement() -> Planned {
        Planned {
            relation: "public.jobs".into(),
            sql: "ALTER TABLE public.jobs\n  ADD COLUMN note text;".into(),
        }
    }

    #[test]
    fn an_empty_plan_is_current() {
        assert_eq!(verdict(&plan(vec![], vec![], vec![])), Verdict::Current);
        assert_eq!(Verdict::Current.exit_code(), 0);
    }

    #[test]
    fn statements_alone_are_additive() {
        assert_eq!(
            verdict(&plan(vec![statement()], vec![], vec![])),
            Verdict::Additive
        );
        assert_eq!(Verdict::Additive.exit_code(), 0);
    }

    #[test]
    fn ahead_columns_do_not_make_a_plan_unsafe() {
        // The whole basis of serving one store from two versions: an older
        // binary reads an ahead store correctly, because no statement uses
        // `SELECT *` (E011). Were this `breaking`, a rollback would be a
        // refusal rather than a rollback.
        assert_eq!(
            verdict(&plan(vec![], vec![], vec![diff(Severity::Ahead)])),
            Verdict::Current
        );
        assert_eq!(
            verdict(&plan(
                vec![statement()],
                vec![],
                vec![diff(Severity::Ahead)]
            )),
            Verdict::Additive
        );
    }

    #[test]
    fn breaking_dominates_a_plan_that_also_has_safe_statements() {
        // Applying the safe half would leave the store between two shapes, so
        // a mixed plan is not "partly fine".
        let mixed = plan(vec![statement()], vec![diff(Severity::Breaking)], vec![]);
        assert_eq!(verdict(&mixed), Verdict::Breaking);
        assert_eq!(Verdict::Breaking.exit_code(), 3);
    }

    #[test]
    fn breaking_exits_3_rather_than_1() {
        // So a deploy gate can tell a REFUSAL from a failure to reach the
        // database. Every other error path in the binary already exits 1.
        assert_eq!(Verdict::Breaking.exit_code(), 3);
        assert_ne!(Verdict::Breaking.exit_code(), 1);
    }

    #[test]
    fn the_report_names_the_refused_difference_and_the_remedy() {
        let rendered = render(
            &plan(vec![], vec![diff(Severity::Breaking)], vec![]),
            "abc123def456789",
        );
        assert!(rendered.contains("verdict: breaking"));
        assert!(rendered.contains("integer -> bigint"));
        assert!(rendered.contains("TWO releases"));
        // The hash is truncated for readability but must still be present.
        assert!(rendered.contains("abc123def456"));
    }

    #[test]
    fn the_report_distinguishes_ahead_from_breaking() {
        let rendered = render(
            &plan(vec![], vec![], vec![diff(Severity::Ahead)]),
            "abc123def456789",
        );
        assert!(rendered.contains("verdict: current"));
        assert!(rendered.contains("AHEAD"));
        assert!(!rendered.contains("REFUSES"));
    }

    #[test]
    fn multi_line_ddl_is_collapsed_so_the_report_stays_scannable() {
        let rendered = render(&plan(vec![statement()], vec![], vec![]), "abc123def456789");
        assert!(rendered.contains("ALTER TABLE public.jobs ADD COLUMN note text;"));
    }

    #[test]
    fn a_very_long_statement_is_truncated_rather_than_flooding_the_report() {
        // The history prelude is a whole plpgsql function: on an empty store it
        // rendered as one ~1500-character line that pushed the other
        // twenty-one statements off the screen.
        let long = Planned {
            relation: "public.er_history()".into(),
            sql: "CREATE OR REPLACE FUNCTION f() ".repeat(80),
        };
        let rendered = render(&plan(vec![long], vec![], vec![]), "abc123def456789");
        let line = rendered
            .lines()
            .find(|l| l.contains("er_history()"))
            .expect("the statement is in the report");
        assert!(
            line.chars().count() < 200,
            "line was {} chars",
            line.chars().count()
        );
        assert!(line.contains("chars)"), "the full length is still reported");
    }

    #[test]
    fn truncation_does_not_split_a_multibyte_character() {
        // The generated DDL carries comments with non-ASCII punctuation, and a
        // byte slice would panic mid-character.
        let long = Planned {
            relation: "public.x".into(),
            sql: "—".repeat(400),
        };
        let rendered = render(&plan(vec![long], vec![], vec![]), "abc");
        assert!(rendered.contains("chars)"));
    }

    #[test]
    fn the_json_carries_every_field_a_gate_branches_on() {
        let value = as_json(
            &plan(
                vec![statement()],
                vec![diff(Severity::Breaking)],
                vec![diff(Severity::Ahead)],
            ),
            "abc123",
        );
        assert_eq!(value["verdict"], "breaking");
        assert_eq!(value["catalog_hash"], "abc123");
        assert_eq!(value["statements"].as_array().unwrap().len(), 1);
        assert_eq!(value["breaking"].as_array().unwrap().len(), 1);
        assert_eq!(value["ahead"].as_array().unwrap().len(), 1);
        assert_eq!(value["ahead"][0]["severity"], "ahead");
    }

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn no_arguments_means_apply() {
        let parsed = parse_migrate_args(&[]).unwrap();
        assert!(!parsed.plans(), "a bare migrate must not plan");
        assert!(!parsed.plan_only);
        assert!(!parsed.expect_additive);
    }

    #[test]
    fn each_mode_parses() {
        assert!(parse_migrate_args(&args(&["--plan"])).unwrap().plan_only);
        assert!(
            parse_migrate_args(&args(&["--expect-additive"]))
                .unwrap()
                .expect_additive
        );
        assert!(
            parse_migrate_args(&args(&["--plan", "--json"]))
                .unwrap()
                .json
        );
        // Both plan, which is what decides whether a plan is computed at all.
        assert!(parse_migrate_args(&args(&["--plan"])).unwrap().plans());
        assert!(
            parse_migrate_args(&args(&["--expect-additive"]))
                .unwrap()
                .plans()
        );
    }

    #[test]
    fn an_unknown_argument_is_refused_and_named() {
        // The case that matters, and it is not hypothetical. Probing the Helm
        // hook against a cluster running an image built before this flag
        // existed, the old binary — which took no arguments — accepted
        // `--does-not-exist` and exited 0 having done a normal migrate. A
        // misspelled `--expect-aditive` in a chart would have turned the gate
        // silently back into a plain apply.
        let err = parse_migrate_args(&args(&["--expect-aditive"])).unwrap_err();
        assert!(
            err.contains("--expect-aditive"),
            "names the offender: {err}"
        );
        assert!(
            err.contains("--expect-additive"),
            "names the alternatives: {err}"
        );
    }

    #[test]
    fn an_unknown_argument_beside_a_valid_one_still_refuses() {
        // Order must not matter: a scan that stopped at the first RECOGNISED
        // flag would accept `--plan --oops`.
        assert!(parse_migrate_args(&args(&["--plan", "--oops"])).is_err());
        assert!(parse_migrate_args(&args(&["--oops", "--plan"])).is_err());
    }

    #[test]
    fn a_short_hash_does_not_panic_the_renderer() {
        // `catalog_hash` is a sha256 so this cannot happen in production, but a
        // slice panic in the gate that refuses a bad deploy would be the worst
        // possible place for one.
        assert!(render(&plan(vec![], vec![], vec![]), "ab").contains("verdict: current"));
        assert!(render(&plan(vec![], vec![], vec![]), "").contains("verdict: current"));
    }
}
