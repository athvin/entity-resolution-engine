//! V11–V13 — what each store and each generated target will actually honour.
//!
//! These were previously uncoded prose inside `validate`. They get codes here
//! because `--explain` and the generated reference need something to key on, and
//! because a reviewer citing "the lake default ban" should be able to cite V11.

use er_entity_spec::{Entity, Language, Store};

use crate::{Code, EntityRule, RuleInfo, Severity, Violation};

const V11_GOOD: &str = r#"generate:
  stores: [duckdb]
  languages: [rust]
columns:
  - name: status
    type: varchar
    nullable: false
    description: "…"
"#;

const V11_BAD: &str = r#"generate:
  stores: [duckdb]
  languages: [rust]
columns:
  - name: status
    type: varchar
    nullable: false
    default: "'active'"
    accepted_values: [active, closed]
    description: "…"
"#;

const V12_GOOD: &str = r"generate:
  stores: [duckdb]
  languages: [rust, typescript]
  wire: [arrow, json]
";

const V12_BAD: &str = r"generate:
  stores: [duckdb]
  languages: [typescript]
  wire: [json]
";

const V13_GOOD: &str = r"generate:
  stores: [postgres]
  languages: [rust]
  history: true
";

const V13_BAD: &str = r"generate:
  stores: [duckdb]
  languages: [rust]
  history: true
";

/// V11 — a lake entity declares nothing the lake will not enforce.
pub struct LakeEnforcesOnlyNotNull;

impl EntityRule for LakeEnforcesOnlyNotNull {
    fn info(&self) -> RuleInfo {
        RuleInfo {
            code: Code("V11"),
            title: "a DuckLake entity declares no default, reference, `accepted_values`, \
                    `serial` or index",
            explain: "DuckLake enforces NOT NULL and nothing else — no defaults, no foreign \
                      keys, no CHECK constraints, no indexes. Declaring one would generate DDL \
                      the lake silently ignores, which is worse than an error: the YAML would \
                      promise a guarantee no layer actually provides, and a reader would be \
                      entitled to rely on it. Control-plane features belong on \
                      `stores: [postgres]` entities.",
            conforming: V11_GOOD,
            violating: V11_BAD,
            severity: Severity::Error,
        }
    }

    fn check(&self, entity: &Entity) -> Vec<Violation> {
        if !entity.has_store(Store::Duckdb) {
            return Vec::new();
        }
        let mut out = Vec::new();
        for column in &entity.columns {
            let mut declared = Vec::new();
            if column.default.is_some() {
                declared.push("default");
            }
            if column.references.is_some() {
                declared.push("references");
            }
            if column.accepted_values.is_some() {
                declared.push("accepted_values");
            }
            if column.serial {
                declared.push("serial");
            }
            if !declared.is_empty() {
                out.push(Violation::new(
                    &entity.name,
                    format!(
                        "column {:?} carries the control-plane feature(s) {} but the entity is \
                         on the lake, which enforces only NOT NULL",
                        column.name,
                        declared.join("/")
                    ),
                ));
            }
        }
        if !entity.indexes.is_empty() {
            out.push(Violation::new(
                &entity.name,
                "indexes are a control-plane feature; the lake has none",
            ));
        }
        out
    }
}

/// V12 — a wire needs a Rust reader.
pub struct WireRequiresRust;

impl EntityRule for WireRequiresRust {
    fn info(&self) -> RuleInfo {
        RuleInfo {
            code: Code("V12"),
            title: "an entity with an arrow or json `wire` also generates for `rust`",
            explain: "A wire format is how this entity crosses a process boundary, and every \
                      such boundary in this system has Rust on at least one end — the engine \
                      writes the Arrow batch, the server serialises the JSON. Declaring a wire \
                      without Rust describes a shape nothing can read or produce.",
            conforming: V12_GOOD,
            violating: V12_BAD,
            severity: Severity::Error,
        }
    }

    fn check(&self, entity: &Entity) -> Vec<Violation> {
        if entity.generate.wire.is_empty() || entity.has_language(Language::Rust) {
            return Vec::new();
        }
        vec![Violation::new(
            &entity.name,
            "a wire (arrow/json) requires generate.languages to include rust",
        )]
    }
}

/// V13 — `history: true` is control-plane only.
pub struct HistoryIsControlPlaneOnly;

impl EntityRule for HistoryIsControlPlaneOnly {
    fn info(&self) -> RuleInfo {
        RuleInfo {
            code: Code("V13"),
            title: "`history: true` appears only on an entity with a `postgres` store",
            explain: "Row-image history is implemented by a Postgres AFTER trigger writing into \
                      a generated `<relation>_history` companion. The lake has no triggers — its \
                      history is DuckLake snapshots, which it keeps whether or not anything is \
                      declared. So `history: true` on a lake entity cannot be honoured, and \
                      asking for it suggests the author expected a companion table that will \
                      never exist.",
            conforming: V13_GOOD,
            violating: V13_BAD,
            severity: Severity::Error,
        }
    }

    fn check(&self, entity: &Entity) -> Vec<Violation> {
        if !entity.generate.history || entity.has_store(Store::Postgres) {
            return Vec::new();
        }
        vec![Violation::new(
            &entity.name,
            "history: true is control-plane only (lake history is DuckLake snapshots)",
        )]
    }
}
