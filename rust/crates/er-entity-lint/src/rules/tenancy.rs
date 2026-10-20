//! V15 — tenant-scoped tables are indexed for tenant-scoped reads.
//!
//! The first rule written against the registry rather than ported into it, and
//! the first at `Severity::Warning`. It reports findings the committed tree has
//! not fixed yet, which the previous fail-fast design could not express at all:
//! a rule was either absent or it blocked every command in the repo. See
//! `docs/entity-lint-rules.md` for the current count.

use er_entity_spec::{Entity, Store};

use crate::{Code, EntityRule, RuleInfo, Severity, Violation};

const V15_GOOD: &str = r#"columns:
  - name: org_id
    type: varchar
    nullable: false
    references: { relation: orgs, column: id }
    description: "…"
indexes:
  - name: staged_pending
    columns: [org_id, state]
"#;

const V15_BAD: &str = r#"columns:
  - name: org_id
    type: varchar
    nullable: false
    references: { relation: orgs, column: id }
    description: "…"
# …and either no indexes at all, or only partial ones:
indexes:
  - name: jobs_one_active_per_org
    columns: [org_id]
    unique: true
    where: "state IN ('running')"
"#;

/// V15 — an `org_id` column comes with an index that can serve a lookup on it.
pub struct TenantScopeIsIndexed;

impl EntityRule for TenantScopeIsIndexed {
    fn info(&self) -> RuleInfo {
        RuleInfo {
            code: Code("V15"),
            title: "a control-plane entity with an `org_id` column declares a non-partial index \
                    leading with `org_id`",
            explain: "`org_id` is how the control plane scopes a row to a tenant, so essentially \
                      every read of such a table filters on it. Postgres can only use an index \
                      for that filter if `org_id` is the index's leading column — a composite \
                      like `[org_id, state]` serves it, `[state, org_id]` does not — and only if \
                      the index is not partial, since a `where` predicate narrows it to rows the \
                      general lookup cannot assume. Without one, every tenant-scoped query is a \
                      sequential scan over every tenant's rows, which degrades as tenants are \
                      added rather than as any one tenant grows. Note that a unique partial \
                      index such as `jobs_one_active_per_org` enforces a real invariant and \
                      should stay: this rule asks for a plain index in addition, not instead.",
            conforming: V15_GOOD,
            violating: V15_BAD,
            severity: Severity::Warning,
        }
    }

    fn check(&self, entity: &Entity) -> Vec<Violation> {
        // Lake entities have no org column and no indexes (V11) — tenant
        // isolation there is catalog separation, not a row predicate.
        if !entity.has_store(Store::Postgres) || entity.column("org_id").is_none() {
            return Vec::new();
        }
        let usable = entity.indexes.iter().any(|index| {
            index.r#where.is_none() && index.columns.first().map(String::as_str) == Some("org_id")
        });
        if usable {
            return Vec::new();
        }
        vec![Violation::new(
            &entity.name,
            "org_id is tenant scope but no non-partial index leads with it, so every \
             tenant-scoped read is a full scan",
        )]
    }
}
