//! `dbt/models/sources.yml`, rendered from the S5 registry (port of
//! `src/er/lake/dbt_sources.py`, DesignDoc.md S5/S5.0/S12).
//!
//! DuckLake enforces `NOT NULL` and nothing else (S5.0), so every logical key is a
//! dbt test or it is nothing. This renders the registry into one source `lake`
//! declaring every `ddl.py`-owned relation: `not_null` per `NOT NULL` column,
//! `accepted_values` per `∈ {…}` domain, `unique` /
//! `dbt_utils.unique_combination_of_columns` per logical key (with S5.0's `where`
//! filter where the key is partial), and the referential `relationships` tests.
//!
//! The document is compared byte for byte against the committed file, so the
//! layout is emitted at fixed depths rather than through a YAML serialiser — a
//! serialiser's formatting choices would be part of the contract too. The header
//! text (which names the Python renderer) is reproduced verbatim so the output
//! stays byte-identical to the committed `dbt/models/sources.yml`; it is re-owned
//! when the Python module is retired.

use crate::model::{Column, Owner, TableSpec};

/// The dbt source every `ddl.py`-owned relation is declared under.
pub const SOURCE_NAME: &str = "lake";
/// S5: the attached catalog alias.
pub const SOURCE_DATABASE: &str = "lake";
/// S5: the schema every relation lives in.
pub const SOURCE_SCHEMA: &str = "main";
/// Where the rendered document is committed, relative to the repository root.
pub const SOURCES_YML: &str = "dbt/models/sources.yml";

/// T-KEY-1a's selector (S12 M1). Every logical-key and domain test carries it.
pub const KEYS_TAG: &str = "keys";
/// The referential tests — deliberately NOT [`KEYS_TAG`]: a dangling reference is
/// not a key violation, and T-KEY-1a asserts on a key violation.
pub const REFS_TAG: &str = "refs";

const UNIQUE_TEST: &str = "unique";
const NOT_NULL_TEST: &str = "not_null";
const ACCEPTED_VALUES_TEST: &str = "accepted_values";
const RELATIONSHIPS_TEST: &str = "relationships";
const COMBINATION_TEST: &str = "dbt_utils.unique_combination_of_columns";

/// A referential expectation between two `ddl.py`-owned relations. Not part of
/// [`TableSpec`]: S5.0's key model is about identity and DuckLake has no
/// `FOREIGN KEY`; dbt's `relationships` test is the only place it is executable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Relationship {
    pub relation: &'static str,
    pub column: &'static str,
    pub to: &'static str,
    pub field: &'static str,
}

/// M3 is one membership row per record, meaningful only if the entity it names
/// exists. T-INV-1 asserts the stronger (`status='active'`) form, but that needs a
/// run; this is the arm that holds on any lake.
pub const RELATIONSHIPS: [Relationship; 1] = [Relationship {
    relation: "entity_membership",
    column: "entity_id",
    to: "entities",
    field: "entity_id",
}];

const HEADER: [&str; 10] = [
    "# GENERATED FILE -- do not edit by hand.",
    "#",
    "# Rendered from the S5 TableSpec registry by",
    "# `er.lake.dbt_sources.render_sources_yml`. `tests/unit/test_dbt_sources_parity.py`",
    "# asserts byte equality with that renderer, so an edit here is a second authority",
    "# for S5 and fails the unit gate rather than drifting silently.",
    "#",
    "# DuckLake enforces NOT NULL and nothing else (S5.0), so every logical key below is",
    "# a test or it is nothing. The `keys` tag is what T-KEY-1a's",
    "# `dbt test --select tag:keys` selects.",
];

const INDENT: &str = "  ";
const TABLE_DEPTH: usize = 3;
const COLUMN_DEPTH: usize = 5;

fn pad(depth: usize) -> String {
    INDENT.repeat(depth)
}

/// A single-quoted YAML scalar, doubling embedded quotes.
fn single(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

/// A double-quoted YAML scalar (for S5.0 `where` filters, which carry their own
/// single quotes).
fn double(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

/// A flow-style sequence of single-quoted scalars: `['a', 'b']`.
fn flow_list<I, S>(values: I) -> String
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let items: Vec<String> = values.into_iter().map(|v| single(v.as_ref())).collect();
    format!("[{}]", items.join(", "))
}

/// One entry of a `data_tests:` list. `arguments` is an ordered list of
/// already-formatted `key: value` pairs (`arguments:` rather than top-level keys
/// because `dbt-core==1.12.2` defaults `require_generic_test_arguments_property`).
fn test_block(
    depth: usize,
    name: &str,
    arguments: &[(&str, String)],
    tags: &[&str],
    r#where: Option<&str>,
) -> Vec<String> {
    if arguments.is_empty() && tags.is_empty() && r#where.is_none() {
        return vec![format!("{}- {}", pad(depth), name)];
    }
    let mut lines = vec![format!("{}- {}:", pad(depth), name)];
    // Two levels, not one: `- ` already consumed the first, so a mapping nested
    // under the test name has to clear the name itself, not just the sequence dash.
    let body = pad(depth + 2);
    if !arguments.is_empty() {
        lines.push(format!("{body}arguments:"));
        for (key, value) in arguments {
            lines.push(format!("{body}{INDENT}{key}: {value}"));
        }
    }
    if !tags.is_empty() || r#where.is_some() {
        lines.push(format!("{body}config:"));
        if !tags.is_empty() {
            lines.push(format!("{body}{INDENT}tags: {}", flow_list(tags)));
        }
        if let Some(clause) = r#where {
            lines.push(format!("{body}{INDENT}where: {}", double(clause)));
        }
    }
    lines
}

fn relationships_for(spec: &TableSpec, column: &str) -> Vec<&'static Relationship> {
    RELATIONSHIPS
        .iter()
        .filter(|edge| edge.relation == spec.name && edge.column == column)
        .collect()
}

/// Every test attached to one column, in the fixed S5.0 order:
/// `not_null`, `unique`, `accepted_values`, `relationships`.
fn column_tests(spec: &TableSpec, column: &Column) -> Vec<Vec<String>> {
    let depth = COLUMN_DEPTH + 2;
    let mut blocks: Vec<Vec<String>> = Vec::new();
    if !column.nullable {
        blocks.push(test_block(depth, NOT_NULL_TEST, &[], &[], None));
    }
    if let Some(key) = spec
        .keys
        .iter()
        .find(|key| key.columns.len() == 1 && key.columns[0] == column.name)
    {
        blocks.push(test_block(
            depth,
            UNIQUE_TEST,
            &[],
            &[KEYS_TAG],
            key.r#where.as_deref(),
        ));
    }
    if let Some(domain) = spec.enums.get(&column.name) {
        // `BTreeSet` iterates sorted — matching Python's `sorted(domain)`.
        blocks.push(test_block(
            depth,
            ACCEPTED_VALUES_TEST,
            &[("values", flow_list(domain.iter()))],
            &[KEYS_TAG],
            None,
        ));
    }
    for edge in relationships_for(spec, &column.name) {
        blocks.push(test_block(
            depth,
            RELATIONSHIPS_TEST,
            &[
                (
                    "to",
                    double(&format!("source('{SOURCE_NAME}', '{}')", edge.to)),
                ),
                ("field", edge.field.to_string()),
            ],
            &[REFS_TAG],
            None,
        ));
    }
    blocks
}

fn column_block(spec: &TableSpec, column: &Column) -> Vec<String> {
    let mut lines = vec![format!("{}- name: {}", pad(COLUMN_DEPTH), column.name)];
    let tests = column_tests(spec, column);
    if !tests.is_empty() {
        lines.push(format!("{}data_tests:", pad(COLUMN_DEPTH + 1)));
        for block in tests {
            lines.extend(block);
        }
    }
    lines
}

/// The table-level tests: every multi-column logical key of S5.0. A key over the
/// empty tuple is skipped (`model_registry`'s singular "one active row" test).
fn table_tests(spec: &TableSpec) -> Vec<Vec<String>> {
    let depth = TABLE_DEPTH + 2;
    spec.keys
        .iter()
        .filter(|key| key.columns.len() > 1)
        .map(|key| {
            test_block(
                depth,
                COMBINATION_TEST,
                &[("combination_of_columns", flow_list(key.columns.iter()))],
                &[KEYS_TAG],
                key.r#where.as_deref(),
            )
        })
        .collect()
}

fn table_block(spec: &TableSpec) -> Vec<String> {
    let mut lines = vec![
        format!("{}- name: {}", pad(TABLE_DEPTH), spec.name),
        format!("{}columns:", pad(TABLE_DEPTH + 1)),
    ];
    for column in &spec.columns {
        lines.extend(column_block(spec, column));
    }
    let tests = table_tests(spec);
    if !tests.is_empty() {
        lines.push(format!("{}data_tests:", pad(TABLE_DEPTH + 1)));
        for block in tests {
            lines.extend(block);
        }
    }
    lines
}

/// The whole `dbt/models/sources.yml` for the `ddl.py`-owned relations, newline
/// terminated and byte-identical to the committed file. dbt-owned specs are
/// skipped: declaring a relation dbt materialises under a contract as a *source*
/// would tell dbt it is an input it does not build (S5.0, D14). `specs` must be in
/// S5 declaration order (`all_specs()`).
#[must_use]
pub fn render_sources_yml(specs: &[TableSpec]) -> String {
    let mut lines: Vec<String> = HEADER.iter().map(|s| (*s).to_string()).collect();
    lines.extend([
        "version: 2".to_string(),
        String::new(),
        "sources:".to_string(),
        format!("{}- name: {SOURCE_NAME}", pad(1)),
        format!("{}database: {SOURCE_DATABASE}", pad(2)),
        format!("{}schema: {SOURCE_SCHEMA}", pad(2)),
        format!("{}tables:", pad(2)),
    ]);
    for spec in specs {
        if spec.owner == Owner::Ddl {
            lines.extend(table_block(spec));
        }
    }
    lines.join("\n") + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::all_specs;

    #[test]
    fn renders_one_lake_source_header() {
        let yml = render_sources_yml(&all_specs());
        assert!(yml.starts_with("# GENERATED FILE -- do not edit by hand.\n"));
        assert!(yml.contains("\nsources:\n  - name: lake\n    database: lake\n    schema: main\n"));
        assert!(yml.ends_with('\n'));
    }

    #[test]
    fn skips_dbt_owned_relations() {
        let yml = render_sources_yml(&all_specs());
        for spec in all_specs() {
            if spec.owner == Owner::Dbt {
                assert!(
                    !yml.contains(&format!("- name: {}\n", spec.name)),
                    "{} is dbt-owned and must not be a source",
                    spec.name
                );
            }
        }
    }
}
