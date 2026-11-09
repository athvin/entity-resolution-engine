//! The attribute vocabulary, as values rather than scattered string literals.
//!
//! Every name a span or metric may carry is declared here once. This exists for
//! the reason `er-entity-lint` exists: the V-rules used to be prose, and the
//! result was that "the rule reference had to be re-typed by hand into the
//! design doc and the skill references — which is how three documented codes
//! ended up never implemented". A telemetry vocabulary has the identical
//! failure mode — a name typed into a dashboard, a runbook and the code,
//! drifting silently, with nothing failing — so the names are compiled in and
//! the reference is generated from them.
//!
//! Two rules govern what may appear here, both from `infrastructure.md` §10:
//!
//! - **§10.4, the PII rule.** This is a deduplication engine, so names, emails,
//!   phones and addresses *are* the payload. An attribute carries identifiers
//!   and counts, never attribute values. [`Pii::Never`] marks a name that must
//!   not exist, so the tests can assert it.
//! - **§10.3, the cardinality rule.** `tenant` is forbidden as a *metric* label
//!   — tenant count is unbounded by design and the bill scales with
//!   tenants × metrics × resolution. The same name is perfectly fine on a span.
//!   [`Cardinality`] records which side of that line a name sits on.

/// Whether a name may appear as a metric dimension, or only on a span.
///
/// The distinction is a cost control, not a style preference: every distinct
/// label value is a separate time series, so an unbounded dimension multiplies
/// the bill without bound. Per-tenant detail comes from traces and the `runs`
/// table instead (§10.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cardinality {
    /// Drawn from a closed vocabulary that exists in the code, so it is safe as
    /// a metric dimension. The `source` names where that vocabulary lives, so a
    /// reviewer can check the claim rather than trust it.
    Bounded {
        /// How many distinct values the vocabulary admits.
        values: usize,
        /// Where the vocabulary is defined.
        source: &'static str,
    },
    /// Grows with tenants, runs or requests. Spans and logs only — never a
    /// metric dimension.
    Unbounded,
}

/// What kind of data a name is allowed to carry, under the §10.4 rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pii {
    /// An opaque or structural identifier: an org slug, a run id, a record key.
    /// §10.4 permits these explicitly.
    Identifier,
    /// A count, a duration, a byte total.
    Count,
    /// Configuration or schema shape — a statement name, a stage, a relation.
    Config,
    /// **Must not exist.** Declared so a reviewer can see the boundary and the
    /// tests can assert nothing emits it. A record's attribute values live in
    /// the lake and in Postgres, and never in telemetry.
    Never,
}

/// One declared attribute.
#[derive(Debug, Clone, Copy)]
pub struct Attr {
    /// The wire name, e.g. `er.statement`.
    pub name: &'static str,
    /// Why it exists and what it holds.
    pub description: &'static str,
    pub pii: Pii,
    pub cardinality: Cardinality,
}

/// Shorthand for a bounded dimension.
const fn bounded(values: usize, source: &'static str) -> Cardinality {
    Cardinality::Bounded { values, source }
}

// --- SQL attribution --------------------------------------------------------

/// The declared statement's identity: `<entity>.<operation>.<name>`.
pub const STATEMENT: &str = "er.statement";
/// The generator that produced a statement whose text is not a constant.
pub const GENERATOR: &str = "er.generator";
/// The relation a statement reads or writes.
pub const RELATION: &str = "db.collection.name";
/// `duckdb` or `postgresql`.
pub const DB_SYSTEM: &str = "db.system.name";
/// The statement template — bound `$n` placeholders, never a literal.
pub const DB_QUERY_TEXT: &str = "db.query.text";
/// A bounded description of a generated statement, in place of its text.
pub const DB_QUERY_SUMMARY: &str = "db.query.summary";
/// Rows returned or affected.
pub const ROWS: &str = "er.rows";

// --- the unit of work -------------------------------------------------------

/// The tenant, by slug.
pub const ORG: &str = "er.org";
/// The resolution domain within a tenant.
pub const DATASET: &str = "er.dataset";
/// The pipeline run.
pub const RUN_ID: &str = "er.run_id";
/// A `RUN_STAGES` member.
pub const STAGE: &str = "er.stage";
/// `ok` or `error` — the dimension that keeps a histogram honest without
/// carrying the error text.
pub const OUTCOME: &str = "er.outcome";

/// Every attribute this workspace may emit.
///
/// The ordering is the reference document's ordering: attribution first, then
/// the unit of work, then the forbidden names last so they read as a boundary
/// rather than a footnote.
pub const ATTRIBUTES: &[Attr] = &[
    Attr {
        name: STATEMENT,
        description: "The declared statement's identity, `<entity>.<operation>.<name>` — the same triple that names its .sql file. Bounded by the entity catalog, but deliberately NOT a metric dimension: pairing each statement with two outcomes would create hundreds of CloudWatch custom metrics for one histogram, and the count grows with every new .sql file.",
        pii: Pii::Config,
        cardinality: bounded(264, "entities/*/*/*.sql, one per declared statement"),
    },
    Attr {
        name: GENERATOR,
        description: "The function that produced a statement whose SQL is assembled at runtime (er_golden::merge_plans_sql and friends). Carried INSTEAD of the text, because generated SQL can embed config-derived literals.",
        pii: Pii::Config,
        cardinality: Cardinality::Unbounded,
    },
    Attr {
        name: RELATION,
        description: "The relation a statement reads or writes, from the entity declaration.",
        pii: Pii::Config,
        cardinality: bounded(51, "distinct relation names in entities/*/entity.yaml"),
    },
    Attr {
        name: DB_SYSTEM,
        description: "`duckdb` for the lake, `postgresql` for the control plane. The one dimension that genuinely separates two different cost and latency profiles.",
        pii: Pii::Config,
        cardinality: bounded(2, "er_span::Store"),
    },
    Attr {
        name: DB_QUERY_TEXT,
        description: "The statement TEMPLATE, with bound $n placeholders. Safe only for declared statements, which the analyser proves literal-free; gated on ER_TRACE_SQL_TEXT because a run executing hundreds of statements would otherwise push tens of megabytes of identical SQL per trace.",
        pii: Pii::Config,
        cardinality: Cardinality::Unbounded,
    },
    Attr {
        name: DB_QUERY_SUMMARY,
        description: "A compile-time description of a statement whose text cannot be exported — the shape without the values.",
        pii: Pii::Config,
        cardinality: Cardinality::Unbounded,
    },
    Attr {
        name: ROWS,
        description: "Rows returned by a read or affected by a write. A count, which §10.4 permits where the values themselves would not be.",
        pii: Pii::Count,
        cardinality: Cardinality::Unbounded,
    },
    Attr {
        name: ORG,
        description: "The tenant, by slug. Spans and logs only — §10.3 forbids tenant as a metric label because tenant count is unbounded by design.",
        pii: Pii::Identifier,
        cardinality: Cardinality::Unbounded,
    },
    Attr {
        name: DATASET,
        description: "The resolution domain within a tenant. Multiplies the §10.3 problem by datasets-per-org, so it is held to the same rule as `er.org`.",
        pii: Pii::Identifier,
        cardinality: Cardinality::Unbounded,
    },
    Attr {
        name: RUN_ID,
        description: "The pipeline run. The join key from a span back to `runs` and `run_stages`, which is how per-tenant detail is recovered without a metric label.",
        pii: Pii::Identifier,
        cardinality: Cardinality::Unbounded,
    },
    Attr {
        name: STAGE,
        description: "A RUN_STAGES member. Bounded and stable, so it is a legitimate metric dimension.",
        pii: Pii::Config,
        cardinality: bounded(11, "er_schema::model::RUN_STAGES"),
    },
    Attr {
        name: OUTCOME,
        description: "`ok` or `error`. Keeps a histogram separable by success without carrying the error text, which is what would otherwise smuggle a message into a label.",
        pii: Pii::Config,
        cardinality: bounded(2, "er_span::Outcome"),
    },
    // The boundary. These are declared so it is visible, and so a test can
    // assert the workspace emits none of them.
    Attr {
        name: "er.record_value",
        description: "FORBIDDEN. A record's attribute values — names, emails, phones, addresses — are this engine's payload, and §10.4's rule is that telemetry carries identifiers and counts and never values. Declared so the boundary is in the reference rather than only in prose.",
        pii: Pii::Never,
        cardinality: Cardinality::Unbounded,
    },
    Attr {
        name: "url.query",
        description: "FORBIDDEN. `GET /v1/orgs/{org}/golden-records` takes `q`, a contains-search over record values, so the query string is payload. `url.path` is fine — it carries identifiers.",
        pii: Pii::Never,
        cardinality: Cardinality::Unbounded,
    },
    Attr {
        name: "er.user.email",
        description: "FORBIDDEN on a span. The email already crosses to er-server as x-acting-user, but that boundary is in-account Postgres; a span's is an exportable store. Plaintext stays in audit_log and erweb.audit; spans carry a user id and pivot by session.",
        pii: Pii::Never,
        cardinality: Cardinality::Unbounded,
    },
];

#[cfg(test)]
mod tests {
    use super::{ATTRIBUTES, Cardinality, Pii};

    #[test]
    fn catalog_bounds_match_declared_sql_and_distinct_relations() {
        use std::collections::BTreeSet;
        use std::fs;
        use std::path::Path;

        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../entities");
        let mut statements = 0;
        let mut relations = BTreeSet::new();
        for entry in fs::read_dir(root).expect("entity directory") {
            let entity_dir = entry.expect("entity directory entry").path();
            if !entity_dir.is_dir() {
                continue;
            }
            let yaml = entity_dir.join("entity.yaml");
            if !yaml.exists() {
                continue;
            }
            let document = fs::read_to_string(yaml).expect("entity YAML");
            let relation = document
                .lines()
                .find_map(|line| line.strip_prefix("relation: "))
                .expect("top-level relation declaration");
            relations.insert(relation.to_string());
            for operation in ["create", "read", "update", "delete"] {
                let dir = entity_dir.join(operation);
                if !dir.exists() {
                    continue;
                }
                statements += fs::read_dir(dir)
                    .expect("statement directory")
                    .map(|entry| entry.expect("statement directory entry").path())
                    .filter(|path| path.extension().is_some_and(|ext| ext == "sql"))
                    .count();
            }
        }

        for (name, actual) in [
            (super::STATEMENT, statements),
            (super::RELATION, relations.len()),
        ] {
            let attr = ATTRIBUTES
                .iter()
                .find(|attr| attr.name == name)
                .expect("attribute");
            let Cardinality::Bounded { values, .. } = attr.cardinality else {
                panic!("{name} must have a catalog bound");
            };
            assert_eq!(values, actual, "{name} bound drifted from entities/");
        }
    }

    #[test]
    fn every_attribute_is_unique_and_documents_itself() {
        let mut seen = std::collections::BTreeSet::new();
        for attr in ATTRIBUTES {
            assert!(seen.insert(attr.name), "{} declared twice", attr.name);
            assert!(
                !attr.description.is_empty(),
                "{} has no description — the reference is generated from these, so an \
                 empty one ships an empty row",
                attr.name
            );
        }
    }

    #[test]
    fn a_forbidden_attribute_is_never_offered_as_a_metric_dimension() {
        // Pii::Never means "must not exist", so it cannot also be described as a
        // usable bounded dimension — that combination would read as permission.
        for attr in ATTRIBUTES {
            if attr.pii == Pii::Never {
                assert!(
                    matches!(attr.cardinality, Cardinality::Unbounded),
                    "{} is forbidden but declared bounded, which reads as a usable dimension",
                    attr.name
                );
            }
        }
    }

    #[test]
    fn a_bounded_vocabulary_names_where_it_is_defined() {
        // A bound nobody can check is a bound nobody will maintain.
        for attr in ATTRIBUTES {
            if let Cardinality::Bounded { values, source } = attr.cardinality {
                assert!(values > 0, "{} claims zero values", attr.name);
                assert!(
                    !source.is_empty(),
                    "{} claims a bound without naming its source",
                    attr.name
                );
            }
        }
    }
}
