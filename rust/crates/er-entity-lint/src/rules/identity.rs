//! V1–V8 — the identity conventions.
//!
//! Every row in this system is addressed the same way: a column called `id` is
//! the whole primary key, it leads the declaration, and `created_at`/`updated_at`
//! bracket its lifetime. Ported from the fail-fast `validate_identity` these grew
//! up in, with one behavioural change: a rule now reports *every* way an entity
//! breaks it rather than the first, so a new entity converges in one pass instead
//! of one error per run.
//!
//! The worked examples live as consts at column zero, because YAML indentation is
//! load-bearing and a struct-literal field cannot hold it faithfully.

use er_entity_spec::{ColumnType, Entity, Store, singular};

use crate::{Code, EntityRule, RuleInfo, Severity, Violation};

const V1_GOOD: &str = r#"columns:
  - name: id
    type: varchar
    nullable: false
    description: "The row's unique id (UUIDv7, minted at creation)."
"#;

const V1_BAD: &str = r#"columns:
  - name: record_key
    type: varchar
    nullable: false
    description: "No id column at all."
"#;

const V2_GOOD: &str = r"key: [id]
natural_key: [record_key]
";

const V2_BAD: &str = r"key: [record_key]
";

const V3_GOOD: &str = r#"columns:
  - name: id
    type: varchar
    nullable: false
    description: "…"
  - name: record_key
    type: varchar
    description: "…"
"#;

const V3_BAD: &str = r#"columns:
  - name: record_key
    type: varchar
    description: "…"
  - name: id
    type: varchar
    nullable: false
    description: "…"
"#;

const V4_GOOD: &str = r#"  - name: created_at
    type: timestamp
    nullable: false
    description: "…"
  - name: updated_at
    type: timestamp
    nullable: false
    description: "…"
"#;

const V4_BAD: &str = r#"  - name: created_at
    type: timestamp
    description: "Nullable, and no updated_at at all."
"#;

const V5_GOOD: &str = r#"  - name: created_at
    type: timestamp
    nullable: false
    default: "now()"
    volatile: true
    description: "…"
"#;

const V5_BAD: &str = r#"  - name: created_at
    type: timestamp
    nullable: false
    description: "No default, not volatile — but the entity is on postgres."
"#;

const V6_GOOD: &str = r#"  - name: created_at
    type: timestamp
    nullable: false
    description: "Creation stamp (the run's logical timestamp)."
"#;

const V6_BAD: &str = r#"  - name: created_at
    type: timestamp
    nullable: false
    volatile: true
    description: "…"
"#;

const V7_GOOD: &str = r#"  - name: org_id
    type: varchar
    nullable: false
    references: { relation: orgs, column: id }
    description: "…"
"#;

const V7_BAD: &str = r#"  - name: tenant
    type: varchar
    nullable: false
    references: { relation: orgs, column: id }
    description: "…"
"#;

const V8_GOOD: &str = r"    references: { relation: orgs, column: id }
";

const V8_BAD: &str = r"    references: { relation: orgs, column: name }
";

/// V1 — a NOT NULL `id` column exists, and is a type we can actually mint.
pub struct IdColumnExists;

impl EntityRule for IdColumnExists {
    fn info(&self) -> RuleInfo {
        RuleInfo {
            code: Code("V1"),
            title: "an entity declares a NOT NULL `id` column, either varchar or bigint+serial",
            explain: "`id` is the one way any row in the system is addressed, so it cannot be \
                      absent or null. Two shapes are allowed and no more: varchar, holding a \
                      UUIDv7 the application mints or a sha256 text the SQL mints, and bigint \
                      with `serial: true` for a database-minted ledger such as audit_log or \
                      ddl_log. The history trigger keys row images by `id`, so an entity without \
                      one cannot carry history either.",
            conforming: V1_GOOD,
            violating: V1_BAD,
            severity: Severity::Error,
        }
    }

    fn check(&self, entity: &Entity) -> Vec<Violation> {
        let Some(id) = entity.column("id") else {
            return vec![Violation::new(
                &entity.name,
                "an entity must declare an id column",
            )];
        };
        let mut out = Vec::new();
        if id.nullable {
            out.push(Violation::new(&entity.name, "id must be NOT NULL"));
        }
        let minted_by_app_or_sql = id.col_type == ColumnType::Varchar;
        let minted_by_database = id.col_type == ColumnType::Bigint && id.serial;
        if !minted_by_app_or_sql && !minted_by_database {
            out.push(Violation::new(
                &entity.name,
                "id must be varchar (UUIDv7/sha256 text) or bigint with serial: true \
                 (a database-minted ledger)",
            ));
        }
        out
    }
}

/// V2 — the primary key is exactly `[id]`.
pub struct KeyIsId;

impl EntityRule for KeyIsId {
    fn info(&self) -> RuleInfo {
        RuleInfo {
            code: Code("V2"),
            title: "the primary key is exactly `[id]`",
            explain: "Natural identity — what makes two rows the same thing — belongs in \
                      `natural_key`, where MERGE statements read it (E015) and where it can \
                      change without rewriting every foreign key that points here. Keeping the \
                      primary key uniformly `[id]` is what lets the generator emit one row-image \
                      trigger, one by-id read and one FK shape for every entity in the tree.",
            conforming: V2_GOOD,
            violating: V2_BAD,
            severity: Severity::Error,
        }
    }

    fn check(&self, entity: &Entity) -> Vec<Violation> {
        if entity.key == ["id"] {
            return Vec::new();
        }
        vec![Violation::new(
            &entity.name,
            format!("key must be exactly [id], got {:?}", entity.key),
        )]
    }
}

/// V3 — `id` leads the declaration.
pub struct IdLeads;

impl EntityRule for IdLeads {
    fn info(&self) -> RuleInfo {
        RuleInfo {
            code: Code("V3"),
            title: "`id` is the first declared column",
            explain: "Column order is declaration order everywhere downstream — the generated \
                      struct, the DDL, the Arrow schema, the TypeScript interface. Pinning `id` \
                      first gives every one of those a uniform shape, so a reader who knows one \
                      entity can read the next without re-learning where identity lives.",
            conforming: V3_GOOD,
            violating: V3_BAD,
            severity: Severity::Error,
        }
    }

    fn check(&self, entity: &Entity) -> Vec<Violation> {
        // A missing id is V1's finding, not this rule's — stay quiet so the
        // report names one cause rather than two.
        if entity.column("id").is_none() {
            return Vec::new();
        }
        if entity.columns.first().map(|c| c.name.as_str()) == Some("id") {
            return Vec::new();
        }
        vec![Violation::new(
            &entity.name,
            "id must be the first declared column",
        )]
    }
}

/// V4 — `created_at` and `updated_at` exist as NOT NULL timestamps.
pub struct TimestampsExist;

impl EntityRule for TimestampsExist {
    fn info(&self) -> RuleInfo {
        RuleInfo {
            code: Code("V4"),
            title: "an entity declares `created_at` and `updated_at` as NOT NULL timestamps",
            explain: "Every row can answer when it appeared and when it last changed, with no \
                      per-entity exceptions to remember. Universality is the point: it lets the \
                      analyser demand that mutations advance `updated_at` (E014) and lets \
                      `created_at` be treated as immutable (E005) without consulting the entity \
                      first. Nullable would reintroduce exactly the per-row doubt the rule \
                      removes.",
            conforming: V4_GOOD,
            violating: V4_BAD,
            severity: Severity::Error,
        }
    }

    fn check(&self, entity: &Entity) -> Vec<Violation> {
        let mut out = Vec::new();
        for name in ["created_at", "updated_at"] {
            let Some(column) = entity.column(name) else {
                out.push(Violation::new(
                    &entity.name,
                    format!("an entity must declare {name} (timestamp, NOT NULL)"),
                ));
                continue;
            };
            if column.col_type != ColumnType::Timestamp || column.nullable {
                out.push(Violation::new(
                    &entity.name,
                    format!("{name} must be a NOT NULL timestamp"),
                ));
            }
        }
        out
    }
}

/// V5 — control-plane timestamps are wall-clock: defaulted and volatile.
pub struct ControlTimestampsAreWallClock;

impl EntityRule for ControlTimestampsAreWallClock {
    fn info(&self) -> RuleInfo {
        RuleInfo {
            code: Code("V5"),
            title: "a Postgres entity's timestamps carry a default and are `volatile: true`",
            explain: "Control-plane rows are written by the server as things happen, so their \
                      timestamps are wall-clock: `default: \"now()\"` means an INSERT need not \
                      pass one, and `volatile: true` excludes them from determinism comparisons, \
                      which would otherwise fail purely because time passed between two runs. \
                      This is the mirror of V6 — the store decides which of the two a timestamp \
                      is.",
            conforming: V5_GOOD,
            violating: V5_BAD,
            severity: Severity::Error,
        }
    }

    fn check(&self, entity: &Entity) -> Vec<Violation> {
        if !entity.has_store(Store::Postgres) {
            return Vec::new();
        }
        let mut out = Vec::new();
        for name in ["created_at", "updated_at"] {
            let Some(column) = entity.column(name) else {
                continue; // V4's finding.
            };
            if column.default.is_none() || !column.volatile {
                out.push(Violation::new(
                    &entity.name,
                    format!(
                        "control-plane {name} must carry a default (now()) and be volatile \
                         (wall-clock)"
                    ),
                ));
            }
        }
        out
    }
}

/// V6 — lake timestamps are run-logical, never volatile.
pub struct LakeTimestampsAreRunLogical;

impl EntityRule for LakeTimestampsAreRunLogical {
    fn info(&self) -> RuleInfo {
        RuleInfo {
            code: Code("V6"),
            title: "a lake entity's timestamps are not `volatile`",
            explain: "Lake rows are written by a run, from the run's own logical clock, and are \
                      part of the determinism contract: the same inputs must reproduce the same \
                      timestamps, which is exactly what a determinism comparison checks. Marking \
                      one volatile would exempt it from that comparison and quietly surrender \
                      the guarantee. The mirror of V5. (A genuinely wall-clock lake column — an \
                      assembly stamp — is a separate column, free to be volatile.)",
            conforming: V6_GOOD,
            violating: V6_BAD,
            severity: Severity::Error,
        }
    }

    fn check(&self, entity: &Entity) -> Vec<Violation> {
        if entity.has_store(Store::Postgres) {
            return Vec::new();
        }
        let mut out = Vec::new();
        for name in ["created_at", "updated_at"] {
            let Some(column) = entity.column(name) else {
                continue; // V4's finding.
            };
            if column.volatile {
                out.push(Violation::new(
                    &entity.name,
                    format!("lake {name} is run-logical and must not be volatile"),
                ));
            }
        }
        out
    }
}

/// V7 — a foreign-key column is named for what it points at.
pub struct ForeignKeyNaming;

impl EntityRule for ForeignKeyNaming {
    fn info(&self) -> RuleInfo {
        RuleInfo {
            code: Code("V7"),
            title: "a foreign-key column is named `<singular relation>_id`, optionally \
                    role-prefixed",
            explain: "The column name alone should say where it points, so a reader never has to \
                      open the referenced entity to find out. `orgs` is referenced by `org_id`; \
                      a role-prefixed form (`parent_org_id`) is allowed for the case where one \
                      entity references the same relation twice and the names have to differ. \
                      The singular is mechanical — see `singular()` in er-entity-spec.",
            conforming: V7_GOOD,
            violating: V7_BAD,
            severity: Severity::Error,
        }
    }

    fn check(&self, entity: &Entity) -> Vec<Violation> {
        let mut out = Vec::new();
        for column in &entity.columns {
            let Some(reference) = &column.references else {
                continue;
            };
            let expected = format!("{}_id", singular(&reference.relation));
            if column.name != expected && !column.name.ends_with(&format!("_{expected}")) {
                out.push(Violation::new(
                    &entity.name,
                    format!(
                        "foreign-key column {:?} must be named {expected:?} (or \
                         <role>_{expected}) after the relation it references",
                        column.name
                    ),
                ));
            }
        }
        out
    }
}

/// V8 — foreign keys reference `id`.
pub struct ForeignKeyTargetsId;

impl EntityRule for ForeignKeyTargetsId {
    fn info(&self) -> RuleInfo {
        RuleInfo {
            code: Code("V8"),
            title: "a foreign key references `id`, never another column",
            explain: "V2 makes `id` the primary key of every entity, so it is the only column \
                      guaranteed unique and stable enough to be pointed at. Referencing a \
                      natural-key column instead would couple this entity to a value the other \
                      entity is free to redefine.",
            conforming: V8_GOOD,
            violating: V8_BAD,
            severity: Severity::Error,
        }
    }

    fn check(&self, entity: &Entity) -> Vec<Violation> {
        let mut out = Vec::new();
        for column in &entity.columns {
            let Some(reference) = &column.references else {
                continue;
            };
            if reference.column != "id" {
                out.push(Violation::new(
                    &entity.name,
                    format!(
                        "foreign keys reference id, not {}({})",
                        reference.relation, reference.column
                    ),
                ));
            }
        }
        out
    }
}
