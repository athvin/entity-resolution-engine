//! V15/V17 — tenant- and dataset-scoped tables declare their scope properly.
//!
//! V15 was the first rule written against the registry rather than ported into
//! it, and the first at `Severity::Warning`. It reports findings the committed
//! tree has not fixed yet, which the previous fail-fast design could not
//! express at all: a rule was either absent or it blocked every command in the
//! repo. See `docs/entity-lint-rules.md` for the current count. V17 is its
//! dataset analogue, written with the datasets program so every table that
//! gains a `dataset_id` arrives well-formed rather than being ratcheted later.

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

const V17_GOOD: &str = r#"columns:
  - name: org_id
    type: varchar
    nullable: false
    references: { relation: orgs, column: id }
    description: "…"
  - name: dataset_id
    type: varchar
    references: { relation: datasets, column: id }
    description: "…"
indexes:
  - name: things_dataset_state
    columns: [dataset_id, state]
"#;

const V17_BAD: &str = r#"columns:
  - name: dataset_id
    type: varchar
    description: "…"
# no references, no org_id beside it, and no dataset_id-leading index
"#;

/// V17 — a `dataset_id` column is a declared scope, not a loose string.
pub struct DatasetScopeIsDeclared;

impl EntityRule for DatasetScopeIsDeclared {
    fn info(&self) -> RuleInfo {
        RuleInfo {
            code: Code("V17"),
            title: "a control-plane entity with a `dataset_id` column references `datasets(id)`, \
                    keeps `org_id` beside it, and declares a non-partial index leading with \
                    `dataset_id`",
            explain: "`dataset_id` scopes a row to one resolution domain inside a tenant. Three \
                      things keep that scope honest. The reference to `datasets(id)` makes it a \
                      real foreign key rather than a string that drifts when a dataset is \
                      purged. `org_id` stays beside it, denormalized on purpose: handlers \
                      resolve the org first, V15's index keeps that read cheap, and history \
                      rows stay legible without a join through `datasets`. And as with V15, \
                      essentially every read of such a table filters on `dataset_id`, which \
                      Postgres can only serve from a non-partial index whose leading column is \
                      `dataset_id` — a partial unique like one-active-per-dataset enforces an \
                      invariant and should stay, but it cannot serve the general lookup.",
            conforming: V17_GOOD,
            violating: V17_BAD,
            severity: Severity::Warning,
        }
    }

    fn check(&self, entity: &Entity) -> Vec<Violation> {
        if !entity.has_store(Store::Postgres) {
            return Vec::new();
        }
        let Some(column) = entity.column("dataset_id") else {
            return Vec::new();
        };
        let mut out = Vec::new();
        let targets_datasets = column
            .references
            .as_ref()
            .is_some_and(|r| r.relation == "datasets" && r.column == "id");
        if !targets_datasets {
            out.push(Violation::new(
                &entity.name,
                "dataset_id does not declare `references: { relation: datasets, column: id }`, \
                 so the scope is a loose string rather than a foreign key",
            ));
        }
        if entity.column("org_id").is_none() {
            out.push(Violation::new(
                &entity.name,
                "dataset_id without org_id beside it: the tenant scope is resolved first and \
                 must not require a join through datasets",
            ));
        }
        let usable = entity.indexes.iter().any(|index| {
            index.r#where.is_none()
                && index.columns.first().map(String::as_str) == Some("dataset_id")
        });
        if !usable {
            out.push(Violation::new(
                &entity.name,
                "dataset_id is dataset scope but no non-partial index leads with it, so every \
                 dataset-scoped read is a full scan",
            ));
        }
        out
    }
}
