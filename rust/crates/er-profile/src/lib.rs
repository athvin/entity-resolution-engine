//! The dataset profiler's findings pass.
//!
//! A profile is a page of numbers, and a page of numbers is not an answer. The
//! findings pass is what turns one into the other: it reads a computed
//! [`dedupe_core::profile::ColumnShape`] and emits coded, human-readable
//! observations — this column is 70% empty, this one is 95% the same value,
//! this identifier is not unique.
//!
//! The rules are a **registry**, not prose and not a chain of `if`s. Each is a
//! value carrying its own code, title, reasoning and a worked
//! conforming/violating pair, so the generated reference
//! (`.claude/skills/er-engine-ops/references/profile-findings.md`) comes from
//! the same source the profiler runs and the reference cannot drift from the
//! behaviour. That is the pattern `er-entity-lint` established for the V-rules;
//! this is the same shape applied to data rather than to schema.
//!
//! Codes are append-only. A retired rule leaves its code behind rather than
//! freeing it, so a stored finding and a review that cited it keep meaning what
//! they said.

use dedupe_core::blocking_analysis::CandidateKeyEstimate;
use dedupe_core::profile::{ColumnShape, ValueFrequency};

pub mod rules;

/// A rule's identifier, `P` followed by a number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Code(pub &'static str);

impl Code {
    /// The numeric part, for ordering. `P10` must sort after `P9`, which it
    /// does not do as a string.
    #[must_use]
    pub fn number(&self) -> u32 {
        self.0.trim_start_matches('P').parse().unwrap_or(0)
    }
}

impl std::fmt::Display for Code {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

/// How loudly a finding should present.
///
/// Deliberately not a gate: nothing here refuses a corpus. A profile describes
/// what is there, and the decision about whether a 70%-empty column is a defect
/// or simply how that source works belongs to the person who owns the data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// Something that will produce wrong results if left alone.
    Error,
    /// Something that will cost recall, runtime or trust.
    Warning,
    /// Worth knowing, no action implied.
    Info,
}

impl Severity {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Info => "info",
        }
    }
}

/// Everything a rule declares about itself.
///
/// `Clone` because a rule may report at more than one severity: P012 answers
/// "did the column set change" in both directions, and a column ARRIVING is
/// information where one VANISHING changes what can be matched. Cloning the
/// declaration and overriding `severity` keeps that a single rule with a single
/// code — one question, both halves of the answer — rather than splitting it
/// into two codes a reader has to join back together.
#[derive(Clone)]
pub struct RuleInfo {
    pub code: Code,
    /// The invariant, as a single lower-case clause.
    pub title: &'static str,
    /// Why the rule exists and what goes wrong without it.
    pub explain: &'static str,
    /// A column this rule is quiet about.
    pub conforming: &'static str,
    /// A column this rule fires on.
    pub violating: &'static str,
    pub severity: Severity,
}

/// One observation about one subject.
#[derive(Debug, Clone, PartialEq)]
pub struct Finding {
    pub code: Code,
    pub severity: Severity,
    /// The column, or empty for a corpus-level finding.
    pub column_name: String,
    /// One sentence, with its numbers already in it.
    pub message: String,
    /// The measurements behind the message.
    pub detail: serde_json::Value,
}

impl Finding {
    #[must_use]
    pub fn new(
        info: &RuleInfo,
        column_name: impl Into<String>,
        message: impl Into<String>,
        detail: serde_json::Value,
    ) -> Self {
        Self {
            code: info.code,
            severity: info.severity,
            column_name: column_name.into(),
            message: message.into(),
            detail,
        }
    }
}

/// What a column rule gets to look at: the column's shape, plus its most
/// frequent values when they were collected.
///
/// The top values are optional because they are a second pass. A rule that
/// needs them must say so by returning nothing when they are absent, rather
/// than guessing — a finding derived from values nobody counted would be a
/// confident answer to a question that was never asked.
pub struct ColumnContext<'a> {
    pub shape: &'a ColumnShape,
    pub top_values: &'a [ValueFrequency],
}

impl ColumnContext<'_> {
    /// The non-null, non-blank share of rows read, or 0.0 for an empty read.
    #[must_use]
    pub fn fill_rate(&self) -> f64 {
        self.shape.fill_rate().unwrap_or(0.0)
    }

    /// The column's most frequent value, when values were collected.
    #[must_use]
    pub fn mode(&self) -> Option<&ValueFrequency> {
        self.top_values.iter().find(|v| v.rank == 1)
    }
}

/// A rule that judges one column.
pub trait ColumnRule: Sync {
    fn info(&self) -> RuleInfo;
    fn check(&self, ctx: &ColumnContext<'_>) -> Vec<Finding>;
}

/// What a corpus rule gets to look at: every column, and every candidate
/// blocking key that was measured.
///
/// Separate from [`ColumnContext`] because some questions are not about a
/// column at all. "Is any column a usable blocking key?" cannot be answered one
/// column at a time — the answer depends on the best of them — and a rule that
/// fired per column would say the same thing N times instead of once.
pub struct CorpusContext<'a> {
    pub columns: &'a [(ColumnShape, Vec<ValueFrequency>)],
    /// Candidate keys, as measured. Empty when the tier that measures them did
    /// not run, which a rule must treat as "unknown" rather than "none viable".
    pub candidate_keys: &'a [CandidateKeyEstimate],
    /// The previous profile of this same corpus, when there is one.
    ///
    /// `Option` rather than an empty collection, for the reason the empty slice
    /// above has a comment: "no previous profile" and "a previous profile with
    /// no columns" are different facts, and a drift rule that confused them
    /// would report every column of a first-ever profile as newly appeared.
    pub previous: Option<&'a PreviousProfile>,
}

/// The parts of an earlier profile a drift rule needs.
///
/// Deliberately not a whole `ProfileReport`: the comparison is presence and
/// fill, so carrying the rest would invite a rule to compare numbers that are
/// not comparable across runs — a tier-2 entropy against a tier-1 absence, or
/// an exact distinct count against a sketch.
#[derive(Debug, Clone)]
pub struct PreviousProfile {
    /// When it was taken, so a finding can say *since when* rather than just
    /// "it changed". A drift finding without a date is unactionable: the reader
    /// cannot tell whether to look at today's delivery or last quarter's.
    pub captured_at: String,
    pub columns: Vec<PreviousColumn>,
}

#[derive(Debug, Clone)]
pub struct PreviousColumn {
    pub column_name: String,
    pub row_count: i64,
    pub non_null_count: i64,
    pub blank_count: i64,
}

impl PreviousColumn {
    /// Non-null, non-blank proportion — the same arithmetic as
    /// [`ColumnShape::fill_rate`], including returning `None` for an empty read
    /// rather than asserting 0.0. If these two ever disagree, every drift
    /// finding is measuring one thing against another.
    #[must_use]
    pub fn fill_rate(&self) -> Option<f64> {
        if self.row_count == 0 {
            return None;
        }
        Some((self.non_null_count - self.blank_count) as f64 / self.row_count as f64)
    }
}

impl PreviousProfile {
    /// The named column as it was, or `None` if it did not exist then.
    #[must_use]
    pub fn column(&self, name: &str) -> Option<&PreviousColumn> {
        self.columns.iter().find(|c| c.column_name == name)
    }
}

/// A rule that judges the corpus as a whole.
pub trait CorpusRule: Sync {
    fn info(&self) -> RuleInfo;
    fn check(&self, ctx: &CorpusContext<'_>) -> Vec<Finding>;
}

/// Run every registered rule over every column, in code order within a column.
///
/// Column order follows the input, so a caller that sorted its columns gets
/// sorted findings — this does not re-sort, because the profile's own column
/// order is meaningful (it is the corpus's) and re-sorting here would silently
/// disagree with the table rendered beside it.
#[must_use]
pub fn findings(columns: &[(ColumnShape, Vec<ValueFrequency>)]) -> Vec<Finding> {
    findings_with(columns, &[])
}

/// As [`findings`], plus the corpus-level rules, which need the candidate keys.
///
/// Kept as a second entry point rather than one with an `Option` so that the
/// tier-1 caller cannot accidentally pass nothing and have a corpus rule read
/// that as "no key is viable" — the empty slice means "not measured", and only
/// this signature makes a caller say which it means.
#[must_use]
pub fn findings_with(
    columns: &[(ColumnShape, Vec<ValueFrequency>)],
    candidate_keys: &[CandidateKeyEstimate],
) -> Vec<Finding> {
    findings_against(columns, candidate_keys, None)
}

/// As [`findings_with`], plus the drift rules, which need the previous profile.
///
/// A third entry point for the same reason there is a second: `None` has to
/// mean "there is no previous profile" and be impossible to confuse with "the
/// previous profile was empty". A caller that has not looked passes `None` and
/// gets silence, which is the right answer — "has this changed?" has none when
/// there is nothing to compare against.
#[must_use]
pub fn findings_against(
    columns: &[(ColumnShape, Vec<ValueFrequency>)],
    candidate_keys: &[CandidateKeyEstimate],
    previous: Option<&PreviousProfile>,
) -> Vec<Finding> {
    let mut out = Vec::new();
    for (shape, top_values) in columns {
        let ctx = ColumnContext { shape, top_values };
        for rule in rules::column_rules() {
            out.extend(rule.check(&ctx));
        }
    }
    let corpus = CorpusContext {
        columns,
        candidate_keys,
        previous,
    };
    for rule in rules::corpus_rules() {
        out.extend(rule.check(&corpus));
    }
    out
}

/// Every rule's declaration, in code order — the source both `--explain` and
/// the generated reference read.
#[must_use]
pub fn rule_infos() -> Vec<RuleInfo> {
    let mut infos: Vec<RuleInfo> = rules::column_rules().iter().map(|r| r.info()).collect();
    infos.extend(rules::corpus_rules().iter().map(|r| r.info()));
    infos.sort_by_key(|i| i.code.number());
    infos
}

/// The generated rule reference
/// (`.claude/skills/er-engine-ops/references/profile-findings.md`), rendered
/// from the registry so it cannot drift from what the profiler actually reports.
#[must_use]
pub fn reference_markdown() -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    out.push_str("# Profile findings\n\n");
    out.push_str(
        "> **Generated** from the registry in `rust/crates/er-profile/src/rules/`.\n\
         > Do not hand-edit — `er-profile`'s own test byte-compares this file.\n\n",
    );
    out.push_str(
        "A profile describes a corpus; a finding says what about it is worth acting on. \
         Nothing here refuses or gates a dataset — whether a 70%-empty column is a defect \
         or simply how that source works is the data owner's call, so every rule reports \
         and none blocks.\n\n",
    );
    // Required of any skill reference over 100 lines by
    // `scripts/skills-check.sh`, and emitted from the registry for the same
    // reason the rest of this file is: an index that can disagree with the
    // sections it indexes is worse than no index.
    out.push_str("## Contents\n\n");
    out.push_str("| Code | Severity | Invariant |\n|---|---|---|\n");
    for info in rule_infos() {
        let _ = writeln!(
            out,
            "| {} | {} | {} |",
            info.code,
            info.severity.as_str(),
            info.title
        );
    }
    for info in rule_infos() {
        let _ = write!(
            out,
            "\n## {} — {}\n\n*{}*\n\n{}\n\n**Quiet on:** {}\n\n**Fires on:** {}\n",
            info.code,
            info.title,
            info.severity.as_str(),
            info.explain,
            info.conforming,
            info.violating
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_unique_and_sort_by_number() {
        let infos = rule_infos();
        let mut seen = std::collections::BTreeSet::new();
        for info in &infos {
            assert!(
                seen.insert(info.code.0),
                "{} is registered twice — codes are append-only and unique",
                info.code
            );
        }
        // P10 must sort after P9, which string ordering gets wrong.
        let numbers: Vec<u32> = infos.iter().map(|i| i.code.number()).collect();
        let mut sorted = numbers.clone();
        sorted.sort_unstable();
        assert_eq!(numbers, sorted, "rule_infos must come back in code order");
        assert!(
            numbers.iter().all(|n| *n > 0),
            "every code parses to a number"
        );
    }

    #[test]
    fn every_rule_documents_itself() {
        for info in rule_infos() {
            assert!(!info.title.is_empty(), "{}: title", info.code);
            assert!(
                info.explain.len() > 80,
                "{}: explain prose is too thin to be worth reading",
                info.code
            );
            assert!(!info.conforming.is_empty(), "{}: conforming", info.code);
            assert!(!info.violating.is_empty(), "{}: violating", info.code);
        }
    }
}
