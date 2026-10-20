//! V9, V10, V14 — conventions about identity and names across the whole tree.
//!
//! V9 and V14 need every entity at once (a reference target, a companion-name
//! collision), so they are [`CatalogRule`]s; V10 judges one entity.

use er_entity_spec::{Catalog, Entity, Store};

use crate::{CatalogRule, Code, EntityRule, RuleInfo, Severity, Violation};

const V9_GOOD: &str = r"# with entities/org declaring `relation: orgs` and an `id` column:
    references: { relation: orgs, column: id }
";

const V9_BAD: &str = r"    references: { relation: parents, column: id }  # no such relation
";

const V10_GOOD: &str = r"natural_key: [org_id, source_name]
";

const V10_BAD: &str = r"natural_key: [created_at]
";

const V14_GOOD: &str = r"# orgs declares `history: true`, and nothing else declares `orgs_history`.
";

const V14_BAD: &str = r"# orgs declares `history: true`, and some other entity declares
# `relation: orgs_history`.
";

/// V9 — every reference resolves to a declared Postgres relation and column.
pub struct ReferencesResolve;

impl CatalogRule for ReferencesResolve {
    fn info(&self) -> RuleInfo {
        RuleInfo {
            code: Code("V9"),
            title: "every `references` target is a declared Postgres relation and a real column \
                    of it",
            explain: "References used to be taken on faith: the generator emitted the FK clause \
                      and a typo surfaced as a Postgres error at schema-apply time, in a \
                      different process on a different day. Resolving the target against the \
                      catalog moves that failure to the moment the YAML is edited. This is a \
                      property of the whole tree, so a deliberately partial catalog (the \
                      differ's tests, a focused tool) can load without it and check later.",
            conforming: V9_GOOD,
            violating: V9_BAD,
            severity: Severity::Error,
        }
    }

    fn check(&self, catalog: &Catalog) -> Vec<Violation> {
        let mut out = Vec::new();
        for entity in catalog.entities() {
            for column in &entity.columns {
                let Some(reference) = &column.references else {
                    continue;
                };
                let target = catalog.entities().iter().find(|candidate| {
                    candidate.relation == reference.relation && candidate.has_store(Store::Postgres)
                });
                let Some(target) = target else {
                    out.push(Violation::new(
                        &entity.name,
                        format!(
                            "column {:?} references {}({}), but no Postgres entity declares that \
                             relation",
                            column.name, reference.relation, reference.column
                        ),
                    ));
                    continue;
                };
                if target.column(&reference.column).is_none() {
                    out.push(Violation::new(
                        &entity.name,
                        format!(
                            "column {:?} references {}({}), but the target declares no such \
                             column",
                            column.name, reference.relation, reference.column
                        ),
                    ));
                }
            }
        }
        out
    }
}

/// V10 — a declared `natural_key` is well-formed.
pub struct NaturalKeyIsWellFormed;

impl EntityRule for NaturalKeyIsWellFormed {
    fn info(&self) -> RuleInfo {
        RuleInfo {
            code: Code("V10"),
            title: "a declared `natural_key` names real columns, without repeats, and never \
                    `id`/`created_at`/`updated_at`",
            explain: "`natural_key` answers what makes two rows the same thing, and the analyser \
                      holds MERGE statements to it (E015), so a typo here silently changes what a \
                      merge considers a match. The universal columns are excluded because they \
                      are row identity and bookkeeping, not natural identity — keying on `id` \
                      would make the merge a no-op and keying on a timestamp would make it \
                      nondeterministic. Nullable columns are allowed: a merge identity can be \
                      partial.",
            conforming: V10_GOOD,
            violating: V10_BAD,
            severity: Severity::Error,
        }
    }

    fn check(&self, entity: &Entity) -> Vec<Violation> {
        let mut out = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        for column in &entity.natural_key {
            if !seen.insert(column.as_str()) {
                out.push(Violation::new(
                    &entity.name,
                    format!("natural_key repeats column {column:?}"),
                ));
            }
            if entity.column(column).is_none() {
                out.push(Violation::new(
                    &entity.name,
                    format!("natural_key column {column:?} is not a column"),
                ));
            }
            if matches!(column.as_str(), "id" | "created_at" | "updated_at") {
                out.push(Violation::new(
                    &entity.name,
                    format!(
                        "natural_key may not include {column:?} — the universal columns are row \
                         identity and bookkeeping, not natural identity"
                    ),
                ));
            }
        }
        out
    }
}

/// V14 — a history companion's relation name stays free.
pub struct HistoryCompanionNameIsFree;

impl CatalogRule for HistoryCompanionNameIsFree {
    fn info(&self) -> RuleInfo {
        RuleInfo {
            code: Code("V14"),
            title: "no declared relation collides with a `history: true` entity's \
                    `<relation>_history` companion",
            explain: "`history: true` makes the generator emit a `<relation>_history` table and a \
                      trigger that writes row images into it. If some other entity already \
                      declares that relation, the two definitions of the same table fight: \
                      whichever DDL applies last wins, and the row images land in a shape nothing \
                      expects. Catching the name collision in the catalog is the only place it is \
                      still cheap.",
            conforming: V14_GOOD,
            violating: V14_BAD,
            severity: Severity::Error,
        }
    }

    fn check(&self, catalog: &Catalog) -> Vec<Violation> {
        let mut out = Vec::new();
        for entity in catalog.entities() {
            if !entity.generate.history {
                continue;
            }
            let companion = format!("{}_history", entity.relation);
            if catalog
                .entities()
                .iter()
                .any(|candidate| candidate.relation == companion)
            {
                out.push(Violation::new(
                    &entity.name,
                    format!("history companion {companion:?} collides with a declared relation"),
                ));
            }
        }
        out
    }
}
