//! The rule registry.
//!
//! Adding a rule is three edits and no more: write the value in one of the
//! modules below, add one line to the `ENTITY_RULES` or `CATALOG_RULES` slice in
//! this file (returned by [`entity_rules`] and [`catalog_rules`]), and add a
//! fixture directory under `tests/fixtures/`. Everything else —
//! `er-entity lint`, `--list`, `--explain`, the generated rule reference, and the
//! `verify` gate — reads those two slices.
//!
//! Codes are append-only. A retired rule leaves its code behind rather than
//! freeing it for reuse, so a review that cited V7 keeps meaning what it said.
//! The next free code is **V16**.

use crate::{CatalogRule, EntityRule};

pub mod identity;
pub mod shape;
pub mod store;
pub mod tenancy;

/// Rules that judge one entity on its own, in code order.
static ENTITY_RULES: &[&dyn EntityRule] = &[
    &identity::IdColumnExists,
    &identity::KeyIsId,
    &identity::IdLeads,
    &identity::TimestampsExist,
    &identity::ControlTimestampsAreWallClock,
    &identity::LakeTimestampsAreRunLogical,
    &identity::ForeignKeyNaming,
    &identity::ForeignKeyTargetsId,
    &shape::NaturalKeyIsWellFormed,
    &store::LakeEnforcesOnlyNotNull,
    &store::WireRequiresRust,
    &store::HistoryIsControlPlaneOnly,
    &tenancy::TenantScopeIsIndexed,
];

/// Rules that need the whole tree at once, in code order.
static CATALOG_RULES: &[&dyn CatalogRule] = &[
    &shape::ReferencesResolve,
    &shape::HistoryCompanionNameIsFree,
];

/// Every entity-scoped rule.
#[must_use]
pub fn entity_rules() -> &'static [&'static dyn EntityRule] {
    ENTITY_RULES
}

/// Every whole-tree rule.
#[must_use]
pub fn catalog_rules() -> &'static [&'static dyn CatalogRule] {
    CATALOG_RULES
}
