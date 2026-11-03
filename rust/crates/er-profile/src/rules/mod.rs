//! The findings registry.
//!
//! Adding a rule is three edits and no more: write the value in one of the
//! modules below, add one line to the `COLUMN_RULES` slice in this file, and add
//! a case to `tests/fixtures.rs`. Everything else — the findings pass, the
//! generated reference, and the doc gate — reads that slice.
//!
//! Codes are append-only. A retired rule leaves its code behind rather than
//! freeing it for reuse, so a stored finding keeps meaning what it said.
//! The next free code is **P011**.

use crate::{ColumnRule, CorpusRule};

pub mod blocking;
pub mod completeness;
pub mod distribution;
pub mod typing;

/// Rules that judge one column, in code order.
static COLUMN_RULES: &[&dyn ColumnRule] = &[
    &completeness::ColumnIsEmpty,
    &completeness::ColumnIsMostlyEmpty,
    &distribution::OneValueDominates,
    &distribution::PlaceholderValuesPresent,
    &distribution::NormalizationCollapsesValues,
    &typing::TextHoldsAConsistentType,
    &distribution::ColumnIsConstant,
    &distribution::IdentifierIsNotUnique,
];

/// Rules that need the whole corpus at once, in code order.
static CORPUS_RULES: &[&dyn CorpusRule] =
    &[&blocking::NoViableBlockingKey, &blocking::BestBlockingKey];

/// Every column-scoped rule.
#[must_use]
pub fn column_rules() -> &'static [&'static dyn ColumnRule] {
    COLUMN_RULES
}

/// Every corpus-scoped rule.
#[must_use]
pub fn corpus_rules() -> &'static [&'static dyn CorpusRule] {
    CORPUS_RULES
}
