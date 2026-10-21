//! `er-entity-lint` — the entity-convention rules as a registry.
//!
//! The conventions every `entities/*/entity.yaml` must hold (the V-rules) used to
//! live as a hand-rolled fail-fast function inside `er-entity-spec`, where a code
//! was a string prefix inside a prose message, only the first violation was ever
//! reported, and the rule reference had to be re-typed by hand into the design doc
//! and the skill references — which is how three documented codes ended up never
//! implemented.
//!
//! Here a rule is a value. It carries its [`Code`], title, explain prose and a
//! worked example pair ([`RuleInfo`]), and it reports [`Violation`]s without
//! knowing its own code — the harness stamps that, so a code can never disagree
//! with itself. [`lint_catalog`] runs every rule against every entity and
//! collects *all* findings, and [`rules()`] is the one list that `--list`,
//! `--explain` and the generated rule reference all read.
//!
//! Adding a rule is: one value in a [`mod@rules`] submodule, one line in the
//! registry, one fixture directory under `tests/fixtures/`.
//!
//! Two rule shapes, because two are what the conventions actually need:
//! [`EntityRule`] sees one entity (V1–V8, V10–V13, V15, V16) and [`CatalogRule`] sees
//! the whole tree (V9, V14).

use std::fmt;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use er_entity_spec::{Catalog, Entity, SpecError};

pub mod rules;

pub use rules::{catalog_rules, entity_rules};

/// A rule's stable identifier. Codes are append-only and never renumbered: a
/// retired rule leaves its code behind rather than freeing it for reuse, so
/// review history keeps meaning what it said.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Code(pub &'static str);

impl Code {
    /// Sort by letter prefix then by number, so V2 precedes V10. Ordinary
    /// string ordering would not, and every list a reader sees is in code order.
    fn sort_key(self) -> (&'static str, u32) {
        let split = self
            .0
            .find(|c: char| c.is_ascii_digit())
            .unwrap_or(self.0.len());
        let (prefix, digits) = self.0.split_at(split);
        (prefix, digits.parse().unwrap_or(0))
    }
}

impl Ord for Code {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.sort_key().cmp(&other.sort_key())
    }
}

impl PartialOrd for Code {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for Code {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // `pad`, not `write_str`: the `--list` table formats this with a width.
        f.pad(self.0)
    }
}

/// Whether a violation fails the gate or merely reports.
///
/// `Error` is the default and what every ported V-rule uses: the conventions are
/// load-bearing for the generator, so a tree that breaks one cannot be generated
/// from. `Warning` exists for a convention we want surfaced before the tree
/// conforms to it — a severity the old fail-fast design had no way to express.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    /// Fails `er-entity verify` and blocks generation.
    Error,
    /// Reported, but the gate still passes.
    Warning,
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // `pad`, not `write_str`: the `--list` table formats this with a width.
        f.pad(match self {
            Self::Error => "error",
            Self::Warning => "warning",
        })
    }
}

/// Everything a rule knows about itself. This is the single source for the
/// `--list` table, `--explain` output and the generated rule reference, so none
/// of the three can drift from the rule that enforces it.
#[derive(Debug, Clone, Copy)]
pub struct RuleInfo {
    /// The stable code (`V4`).
    pub code: Code,
    /// A one-line statement of the invariant, in the present tense.
    pub title: &'static str,
    /// Why the rule exists — the reasoning a reader needs to accept it, not a
    /// restatement of the title.
    pub explain: &'static str,
    /// A YAML fragment that satisfies the rule.
    pub conforming: &'static str,
    /// A YAML fragment that breaks it.
    pub violating: &'static str,
    /// Whether a violation fails the gate.
    pub severity: Severity,
}

/// What a rule reports: the offending entity and what is wrong with it. The rule
/// deliberately does not name its own code — [`lint_catalog`] stamps that from
/// [`RuleInfo`], so the two cannot disagree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    /// The entity the finding is about.
    pub entity: String,
    /// One line, lowercase, naming what is wrong.
    pub message: String,
}

impl Violation {
    /// Report a finding against an entity.
    pub fn new(entity: &str, message: impl Into<String>) -> Self {
        Self {
            entity: entity.to_string(),
            message: message.into(),
        }
    }
}

/// A [`Violation`] with its rule's identity and the file it belongs to attached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    /// The rule that fired.
    pub code: Code,
    /// Whether this fails the gate.
    pub severity: Severity,
    /// The offending entity's name.
    pub entity: String,
    /// The `entity.yaml` the finding is about.
    pub path: PathBuf,
    /// One line naming what is wrong.
    pub message: String,
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "[{}] {}: {}",
            self.code,
            self.path.display(),
            self.message
        )
    }
}

/// A rule that judges one entity on its own.
pub trait EntityRule: Sync {
    /// The rule's identity and prose.
    fn info(&self) -> RuleInfo;
    /// Every way this entity breaks the rule. Empty when it holds, and empty
    /// when the rule does not apply to this entity at all.
    fn check(&self, entity: &Entity) -> Vec<Violation>;
}

/// A rule that needs the whole tree — cross-entity references, name collisions.
pub trait CatalogRule: Sync {
    /// The rule's identity and prose.
    fn info(&self) -> RuleInfo;
    /// Every way this catalog breaks the rule.
    fn check(&self, catalog: &Catalog) -> Vec<Violation>;
}

/// Every registered rule in code order.
///
/// Sorted rather than concatenated: which of the two registries a rule lives in
/// is an implementation detail of how it is checked, and no reader of `--list` or
/// the generated reference should have to know it to find V9.
#[must_use]
pub fn rules() -> Vec<RuleInfo> {
    let mut all: Vec<RuleInfo> = entity_rules()
        .iter()
        .map(|rule| rule.info())
        .chain(catalog_rules().iter().map(|rule| rule.info()))
        .collect();
    all.sort_by_key(|info| info.code);
    all
}

/// Look a rule up by code, for `er-entity lint --explain`. Case-insensitive, so
/// `v4` and `V4` both resolve.
#[must_use]
pub fn explain(code: &str) -> Option<RuleInfo> {
    rules()
        .into_iter()
        .find(|info| info.code.0.eq_ignore_ascii_case(code))
}

/// The path a finding about `entity` points at. An entity lives in the directory
/// named for it, which is also how [`Catalog::load_dir`] finds it.
fn entity_path(entities_dir: &Path, entity: &str) -> PathBuf {
    entities_dir.join(entity).join("entity.yaml")
}

/// Run every rule over the catalog and collect **all** findings, sorted by
/// entity then code so the output is stable across runs and platforms.
///
/// `entities_dir` is the `entities/` root the catalog was loaded from; it is used
/// only to build the paths diagnostics point at.
#[must_use]
pub fn lint_catalog(catalog: &Catalog, entities_dir: &Path) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    for rule in entity_rules() {
        let info = rule.info();
        for entity in catalog.entities() {
            for violation in rule.check(entity) {
                out.push(stamp(&info, violation, entities_dir));
            }
        }
    }
    for rule in catalog_rules() {
        let info = rule.info();
        for violation in rule.check(catalog) {
            out.push(stamp(&info, violation, entities_dir));
        }
    }
    out.sort_by(|a, b| {
        a.entity
            .cmp(&b.entity)
            .then(a.code.cmp(&b.code))
            .then(a.message.cmp(&b.message))
    });
    out
}

fn stamp(info: &RuleInfo, violation: Violation, entities_dir: &Path) -> Diagnostic {
    Diagnostic {
        code: info.code,
        severity: info.severity,
        path: entity_path(entities_dir, &violation.entity),
        entity: violation.entity,
        message: violation.message,
    }
}

/// A load-and-lint failure.
#[derive(Debug, thiserror::Error)]
pub enum LintError {
    /// The catalog itself could not be loaded or is structurally incoherent.
    // The cause is reached through `.source()`; interpolating it here would
    // double-print it in every reporter.
    #[error("loading the entity catalog")]
    Load {
        #[from]
        source: SpecError,
    },
    /// The catalog loaded, but one or more rules failed at `Severity::Error`.
    /// The diagnostics are carried rather than formatted so the caller can print
    /// them in its own shape.
    #[error("{} entity lint error(s)", diagnostics.len())]
    Rules {
        /// Every error-severity finding, in report order.
        diagnostics: Vec<Diagnostic>,
    },
}

/// Load an `entities/` tree and lint it: the one entry point that guarantees a
/// catalog has been held to the conventions.
///
/// Prefer this over [`Catalog::load_dir`] anywhere a generated artifact is about
/// to be written: `load_dir` enforces only what makes a [`Catalog`] coherent, and
/// the conventions the generator relies on are checked here.
///
/// Only error-severity findings fail. Warnings are deliberately *not* returned —
/// a caller about to generate has nothing to do with them, and threading them out
/// here would have every command print the same list. Call [`lint_catalog`]
/// directly to report them; `er-entity lint` does.
pub fn load_linted(entities_dir: &Path) -> Result<Catalog, LintError> {
    let catalog = Catalog::load_dir(entities_dir)?;
    let errors: Vec<Diagnostic> = lint_catalog(&catalog, entities_dir)
        .into_iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    if errors.is_empty() {
        Ok(catalog)
    } else {
        Err(LintError::Rules {
            diagnostics: errors,
        })
    }
}

/// Render the rule reference as markdown, for `er-entity lint --docs`.
///
/// `er-entity verify` byte-compares this against the committed
/// `docs/entity-lint-rules.md`, the same way it does the generated ERD — which is
/// what keeps the documented rule set and the enforced one identical.
#[must_use]
pub fn rules_markdown() -> String {
    let mut out = String::new();
    out.push_str("# Entity lint rules\n\n");
    out.push_str(
        "Generated by `er-entity lint --docs` from the `er-entity-lint` registry — \
         do not hand-edit.\n`er-entity verify` fails if this file and the registry \
         disagree.\n\nThese are the **entity** conventions, checked against \
         `entities/*/entity.yaml`. The\n**statement** diagnostics (`E0xx`), which need a \
         query planner, are a separate family\nin `er-sql-check`.\n\n",
    );

    out.push_str("| Code | Severity | Invariant |\n|---|---|---|\n");
    for info in rules() {
        let anchor = info.code.0.to_lowercase();
        let _ = writeln!(
            out,
            "| [{}](#{anchor}) | {} | {} |",
            info.code, info.severity, info.title
        );
    }

    for info in rules() {
        let _ = write!(
            out,
            "\n## {}\n\n{}\n\n{}\n\nSeverity: **{}**\n\n",
            info.code, info.title, info.explain, info.severity
        );
        out.push_str("Conforming:\n\n```yaml\n");
        out.push_str(info.conforming.trim_matches('\n'));
        out.push_str("\n```\n\nViolating:\n\n```yaml\n");
        out.push_str(info.violating.trim_matches('\n'));
        out.push_str("\n```\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_sort_by_number_not_by_string() {
        // The reason `Code` has a hand-written `Ord`: plain string ordering puts
        // V10 before V2, and every list a reader sees is in code order.
        assert!(Code("V2") < Code("V10"));
        assert!(Code("V9") < Code("V10"));
        assert!(Code("V1") < Code("V2"));
        assert!(Code("V15") > Code("V14"));
        // A different letter family stays grouped rather than interleaving.
        assert!(Code("E9") < Code("V1"));
    }

    #[test]
    fn the_registry_is_listed_in_code_order() {
        let codes: Vec<Code> = rules().iter().map(|info| info.code).collect();
        let mut sorted = codes.clone();
        sorted.sort_unstable();
        assert_eq!(
            codes, sorted,
            "rules() must be in code order regardless of which registry a rule lives in"
        );
    }

    #[test]
    fn every_code_is_unique() {
        let mut codes: Vec<&str> = rules().iter().map(|info| info.code.0).collect();
        let before = codes.len();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), before, "two rules claim the same code");
    }

    #[test]
    fn every_rule_documents_itself() {
        for info in rules() {
            let code = info.code;
            assert!(!info.title.trim().is_empty(), "{code} has no title");
            assert!(
                !info.explain.trim().is_empty(),
                "{code} has no explain prose"
            );
            assert!(
                !info.conforming.trim().is_empty(),
                "{code} has no conforming example"
            );
            assert!(
                !info.violating.trim().is_empty(),
                "{code} has no violating example"
            );
            // A title that restates the code teaches nothing.
            assert!(
                !info.title.starts_with(code.0),
                "{code}'s title should state the invariant, not repeat the code"
            );
        }
    }

    #[test]
    fn explain_is_case_insensitive_and_total_over_the_registry() {
        for info in rules() {
            let code = info.code.0;
            assert!(explain(code).is_some(), "{code} is not explainable");
            assert!(
                explain(&code.to_lowercase()).is_some(),
                "{code} is not explainable in lowercase"
            );
        }
        assert!(explain("V0").is_none());
    }

    #[test]
    fn the_rule_reference_covers_every_registered_rule() {
        let markdown = rules_markdown();
        for info in rules() {
            assert!(
                markdown.contains(&format!("## {}", info.code)),
                "{} is missing from the generated reference",
                info.code
            );
        }
    }
}
