//! `er-entity-gen` — the emitters.
//!
//! Given a validated [`er_entity_spec::Catalog`], this crate produces every
//! generated artifact as deterministic text: the Rust module (struct +
//! `arrow_schema()`), the DuckLake DDL (via `er-schema`'s renderer), and the
//! TypeScript binding and reader. The one shared type map lives in these emitters
//! and nowhere else; the analyser's oracle (a later phase) is what proves the map
//! matches what the databases actually return.

// This crate is a text generator: building output with `push_str(&format!(...))`
// reads more clearly here than threading `write!` and its (impossible) errors
// through every emitter.
#![allow(clippy::format_push_string)]

mod casing;
mod convert;
mod ddl;
mod pg_ddl;
mod rust;
mod typescript;

use std::path::PathBuf;

use er_entity_spec::{Catalog, Entity, Language, Store, Wire};

pub use convert::{ConvertError, to_lake_table_spec};

/// One generated file: a repo-relative path and its full contents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedFile {
    /// The path relative to the repository root.
    pub path: PathBuf,
    /// The file's complete, newline-terminated contents.
    pub contents: String,
}

/// The complete set of files one generator run produces, in a stable order.
#[derive(Debug, Clone, Default)]
pub struct GeneratedTree {
    /// Every generated file.
    pub files: Vec<GeneratedFile>,
}

impl GeneratedTree {
    fn push(&mut self, path: impl Into<PathBuf>, contents: String) {
        self.files.push(GeneratedFile {
            path: path.into(),
            contents,
        });
    }
}

/// Where each kind of artifact is written, relative to the repository root.
const RUST_ENTITIES_DIR: &str = "rust/crates/er-entities/src";
const LAKE_DDL_DIR: &str = "rust/crates/er-entities/ddl/lake";
const PG_DDL_DIR: &str = "rust/crates/er-entities/ddl/postgres";
const TS_BINDINGS_DIR: &str = "frontend/src/lib/entities/bindings";
const TS_READERS_DIR: &str = "frontend/src/lib/entities/readers";

/// Generate every artifact the catalog's entities ask for, as an in-memory tree.
/// Writing it to disk (and the stale-output check) is the CLI's job.
pub fn generate(catalog: &Catalog) -> Result<GeneratedTree, ConvertError> {
    let mut tree = GeneratedTree::default();
    let mut rust_modules: Vec<&str> = Vec::new();

    for entity in catalog.entities() {
        if entity.has_language(Language::Rust) {
            tree.push(
                format!("{RUST_ENTITIES_DIR}/{}.rs", entity.name),
                rust::emit_module(entity),
            );
            rust_modules.push(&entity.name);
        }
        if entity.has_store(Store::Duckdb) {
            tree.push(
                format!("{LAKE_DDL_DIR}/{}.sql", entity.relation),
                format!("{}\n", ddl::emit_lake_ddl(entity)?),
            );
        }
        if entity.has_store(Store::Postgres) {
            tree.push(
                format!("{PG_DDL_DIR}/{}.sql", entity.relation),
                pg_ddl::emit_postgres_ddl(entity)?,
            );
        }
        // The TypeScript binding (the type) is emitted for every entity that lists
        // `typescript`; the Arrow reader additionally for entities on the Arrow
        // wire (the data lane). Control entities (json wire) get a binding only —
        // their control-lane routes are typed through schema.d.ts as well. The
        // generated `src/lib/entities/` directory is exempt from the frontend's
        // strict eslint/prettier, the way the generated Rust crate is exempt from
        // pedantic clippy.
        if entity.has_language(Language::Typescript)
            && (entity.has_wire(Wire::Arrow) || entity.has_wire(Wire::Json))
        {
            let type_name = casing::to_pascal_case(&entity.name);
            tree.push(
                format!("{TS_BINDINGS_DIR}/{type_name}.ts"),
                typescript::emit_binding(entity),
            );
            if entity.has_wire(Wire::Arrow) {
                tree.push(
                    format!("{TS_READERS_DIR}/{type_name}.ts"),
                    typescript::emit_reader(entity),
                );
            }
        }
    }

    // The control-plane baseline: every `public.*` table's DDL, FK-ordered, so a
    // fresh control database can be provisioned from the generated catalog (the
    // er-server boot applier) with no hand-written schema.
    let pg_entities = catalog.for_store(Store::Postgres);
    let mut extra_modules: Vec<&str> = Vec::new();
    if !pg_entities.is_empty() {
        tree.push(
            format!("{RUST_ENTITIES_DIR}/schema.rs"),
            emit_postgres_baseline(&pg_entities)?,
        );
        extra_modules.push("schema");
    }

    // The generated crate root, so `er-entities` is 100% generated: the clippy
    // suppressions a generated crate needs, plus one `pub mod` per entity.
    if !rust_modules.is_empty() {
        rust_modules.extend(extra_modules);
        tree.push(
            format!("{RUST_ENTITIES_DIR}/lib.rs"),
            emit_crate_root(&rust_modules),
        );
    }

    Ok(tree)
}

/// Order entities so a referenced relation precedes the relation that references
/// it (depth-first over column `references`), so `CREATE TABLE` applies without a
/// dangling foreign key. Cycles (none in practice) fall back to first-visit order.
fn topo_order<'a>(entities: &[&'a Entity]) -> Vec<&'a Entity> {
    use std::collections::{BTreeMap, BTreeSet};
    let by_rel: BTreeMap<&str, &'a Entity> =
        entities.iter().map(|e| (e.relation.as_str(), *e)).collect();
    let mut visited = BTreeSet::new();
    let mut ordered = Vec::new();

    fn visit<'a>(
        entity: &'a Entity,
        by_rel: &BTreeMap<&str, &'a Entity>,
        visited: &mut BTreeSet<String>,
        ordered: &mut Vec<&'a Entity>,
    ) {
        if !visited.insert(entity.relation.clone()) {
            return;
        }
        for col in &entity.columns {
            if let Some(reference) = &col.references {
                if let Some(dep) = by_rel.get(reference.relation.as_str()) {
                    visit(dep, by_rel, visited, ordered);
                }
            }
        }
        ordered.push(entity);
    }

    for entity in entities {
        visit(entity, &by_rel, &mut visited, &mut ordered);
    }
    ordered
}

/// Emit the generated `schema` module: the control-plane baseline as a slice of
/// per-entity Postgres DDL strings in FK-dependency order. The er-server boot
/// applier runs these to provision a fresh control database from the catalog.
fn emit_postgres_baseline(entities: &[&Entity]) -> Result<String, ConvertError> {
    let mut out = String::from(
        "// @generated by er-entity-gen — DO NOT EDIT.\n\
         //! The control-plane baseline: every `public.*` table's DDL (CREATE TABLE +\n\
         //! indexes) in foreign-key dependency order, so a fresh control database is\n\
         //! provisioned from the generated catalog — no hand-written schema.\n\n",
    );
    out.push_str(
        "/// Each entry is one entity's full Postgres DDL, ordered so a referenced table\n\
         /// precedes the table that references it. Applied at boot by er-server.\n\
         pub const POSTGRES_BASELINE: &[&str] = &[\n",
    );
    for entity in topo_order(entities) {
        let ddl = pg_ddl::emit_postgres_ddl(entity)?;
        out.push_str("    r#\"");
        out.push_str(ddl.trim_end());
        out.push_str("\"#,\n");
    }
    out.push_str("];\n");
    Ok(out)
}

fn emit_crate_root(modules: &[&str]) -> String {
    let mut out = String::from(
        "// @generated by er-entity-gen — DO NOT EDIT.\n\
         //! The generated entity crate: one module per entity, each with its struct,\n\
         //! `arrow_schema()` and (in later phases) its typed query functions.\n\
         #![allow(clippy::all, clippy::pedantic, clippy::nursery)]\n\n",
    );
    for name in modules {
        out.push_str(&format!("pub mod {name};\n"));
    }
    out
}

/// Emit just the Rust module for one entity (used by tests and the CLI).
#[must_use]
pub fn emit_rust_module(entity: &Entity) -> String {
    rust::emit_module(entity)
}

/// Emit just the TypeScript binding for one entity.
#[must_use]
pub fn emit_ts_binding(entity: &Entity) -> String {
    typescript::emit_binding(entity)
}

/// Emit just the TypeScript reader for one entity.
#[must_use]
pub fn emit_ts_reader(entity: &Entity) -> String {
    typescript::emit_reader(entity)
}

/// Emit just the lake DDL for one entity.
pub fn emit_lake_ddl(entity: &Entity) -> Result<String, ConvertError> {
    ddl::emit_lake_ddl(entity)
}

/// Emit just the Postgres DDL for one control-plane entity.
pub fn emit_postgres_ddl(entity: &Entity) -> Result<String, ConvertError> {
    pg_ddl::emit_postgres_ddl(entity)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The committed `golden_record` entity, compiled in so the oracle test fails
    /// if the YAML ever drifts from the live `er-schema` registry.
    const GOLDEN_YAML: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../entities/golden_record/entity.yaml"
    ));

    fn golden() -> Entity {
        serde_norway::from_str(GOLDEN_YAML).expect("golden_record entity parses")
    }

    #[test]
    fn golden_record_columns_match_the_registry() {
        let spec = to_lake_table_spec(&golden()).expect("converts to a lake spec");
        let registry = er_schema::model::all_specs()
            .into_iter()
            .find(|s| s.name == "golden_records")
            .expect("golden_records is in the registry");
        assert_eq!(
            spec.columns, registry.columns,
            "the golden_record entity's columns must equal the er-schema registry's golden_records"
        );
    }

    /// Every committed lake entity must match the live er-schema registry for the
    /// relation of the same name — the oracle that keeps the entity YAML and the
    /// hand-built registry from drifting as the golden marts move to entity
    /// ownership.
    #[test]
    fn every_lake_entity_matches_the_registry() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../entities");
        let catalog = er_entity_spec::Catalog::load_dir(dir).expect("entities load");
        let specs = er_schema::model::all_specs();
        for entity in catalog.entities() {
            if !entity.has_store(er_entity_spec::Store::Duckdb) {
                continue;
            }
            let spec = to_lake_table_spec(entity).expect("converts to a lake spec");
            let registry = specs
                .iter()
                .find(|s| s.name == entity.relation)
                .unwrap_or_else(|| panic!("relation {} is not in the registry", entity.relation));
            assert_eq!(
                spec.columns, registry.columns,
                "entity {} columns differ from the registry's {}",
                entity.name, entity.relation
            );
        }
    }

    /// The completeness direction of the oracle above: every stage-materialised
    /// registry relation is declared as a lake entity, so a relation can never
    /// again live only in the hand-built registry (the dbt-era shape).
    #[test]
    fn every_stage_relation_is_an_entity() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../entities");
        let catalog = er_entity_spec::Catalog::load_dir(dir).expect("entities load");
        for relation in er_schema::model::stage_owned() {
            let entity = catalog
                .entities()
                .iter()
                .find(|e| e.relation == relation && e.has_store(er_entity_spec::Store::Duckdb));
            assert!(
                entity.is_some(),
                "stage-materialised relation {relation} has no lake entity"
            );
        }
    }

    #[test]
    fn job_postgres_ddl_has_constraints_and_widened_integers() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../entities");
        let catalog = er_entity_spec::Catalog::load_dir(dir).expect("entities load");
        let job = catalog.by_name("job").expect("job entity");
        let ddl = emit_postgres_ddl(job).expect("renders Postgres DDL");
        assert!(ddl.contains("CREATE TABLE IF NOT EXISTS public.jobs ("));
        assert!(ddl.contains("org text NOT NULL REFERENCES orgs(name)"));
        assert!(ddl.contains("params jsonb NOT NULL DEFAULT '{}'::jsonb"));
        // The five 32-bit columns are widened to bigint (the vocabulary has no i32).
        assert!(ddl.contains("priority bigint NOT NULL DEFAULT 0"));
        assert!(ddl.contains("attempt bigint NOT NULL DEFAULT 0"));
        assert!(ddl.contains("max_attempts bigint NOT NULL DEFAULT 3"));
        assert!(ddl.contains("PRIMARY KEY (job_id)"));
        assert!(ddl.contains(
            "CREATE UNIQUE INDEX IF NOT EXISTS jobs_idempotency ON public.jobs (org, idempotency_key) WHERE idempotency_key IS NOT NULL;"
        ));
    }

    #[test]
    fn org_state_renders_an_accepted_values_check() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../entities");
        let catalog = er_entity_spec::Catalog::load_dir(dir).expect("entities load");
        let org = catalog.by_name("org").expect("org entity");
        let ddl = emit_postgres_ddl(org).expect("renders Postgres DDL");
        assert!(ddl.contains(
            "state text NOT NULL DEFAULT 'active' CHECK (state IN ('provisioning', 'active', 'suspended', 'purging'))"
        ));
        assert!(ddl.contains("active_config_version bigint"));
    }

    #[test]
    fn postgres_baseline_is_fk_ordered_and_covers_every_control_table() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../entities");
        let catalog = er_entity_spec::Catalog::load_dir(dir).expect("entities load");
        let pg = catalog.for_store(er_entity_spec::Store::Postgres);
        let baseline = emit_postgres_baseline(&pg).expect("baseline renders");
        // Every control table is present …
        for entity in &pg {
            assert!(
                baseline.contains(&format!(
                    "CREATE TABLE IF NOT EXISTS public.{}",
                    entity.relation
                )),
                "baseline is missing {}",
                entity.relation
            );
        }
        // … and orgs (the FK target of the others) is created before a referrer.
        let orgs = baseline.find("public.orgs (").expect("orgs present");
        let jobs = baseline.find("public.jobs (").expect("jobs present");
        assert!(orgs < jobs, "orgs must precede jobs (it is the FK target)");
    }

    #[test]
    fn audit_log_serial_id_renders_bigserial_but_the_struct_is_i64() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../entities");
        let catalog = er_entity_spec::Catalog::load_dir(dir).expect("entities load");
        let audit = catalog.by_name("audit_log").expect("audit_log entity");
        // Postgres DDL: the serial bigint renders as bigserial.
        let ddl = emit_postgres_ddl(audit).expect("renders Postgres DDL");
        assert!(ddl.contains("id bigserial NOT NULL"), "{ddl}");
        assert!(ddl.contains("PRIMARY KEY (id)"));
        assert!(ddl.contains("detail jsonb NOT NULL DEFAULT '{}'::jsonb"));
        // The struct keeps the plain bigint (i64) — serial is a DDL-only decoration.
        let module = emit_rust_module(audit);
        assert!(module.contains("pub id: i64,"), "{module}");
    }

    #[test]
    fn golden_record_lake_ddl_renders() {
        let ddl = emit_lake_ddl(&golden()).expect("renders DDL");
        assert!(ddl.starts_with("CREATE TABLE IF NOT EXISTS lake.main.golden_records ("));
        assert!(ddl.contains("  entity_id VARCHAR NOT NULL"));
        assert!(ddl.contains("  given_name VARCHAR,"));
        assert!(ddl.contains("  birth_date DATE,"));
        assert!(ddl.contains("  metadata JSON,"));
        assert!(ddl.contains("  survivorship_version VARCHAR NOT NULL"));
        assert!(ddl.contains("  assembled_at TIMESTAMP NOT NULL"));
    }

    #[test]
    fn rust_module_has_struct_and_arrow_schema() {
        let module = emit_rust_module(&golden());
        assert!(module.contains("pub struct GoldenRecord {"));
        assert!(module.contains("pub entity_id: String,"));
        assert!(module.contains("pub given_name: Option<String>,"));
        assert!(module.contains("pub birth_date: Option<::chrono::NaiveDate>,"));
        assert!(module.contains("pub assembled_at: ::chrono::DateTime<::chrono::Utc>,"));
        assert!(module.contains("pub const KEY: &'static [&'static str] = &[\"entity_id\"];"));
        assert!(module.contains("pub fn arrow_schema() -> ::arrow::datatypes::Schema {"));
        assert!(module.contains(
            "Field::new(\"assembled_at\", DataType::Timestamp(TimeUnit::Microsecond, None), false)"
        ));
        assert!(module.contains("\"er.schema_version\".to_string(), \"1\".to_string()"));
    }

    #[test]
    fn ts_binding_maps_the_type_vocabulary() {
        let binding = emit_ts_binding(&golden());
        assert!(binding.contains("export interface GoldenRecord {"));
        assert!(binding.contains("  entity_id: string;"));
        assert!(binding.contains("  given_name: string | null;"));
        assert!(binding.contains("  birth_date: string | null;"));
        assert!(binding.contains("  metadata: unknown;"));
        assert!(binding.contains("  assembled_at: string;"));
    }

    #[test]
    fn ts_reader_checks_version_and_required_columns() {
        let reader = emit_ts_reader(&golden());
        assert!(reader.contains("export function readGoldenRecord(table: Table): GoldenRecord[]"));
        assert!(reader.contains("const EXPECTED_VERSION = \"1\";"));
        assert!(reader.contains("\"entity_id\""));
        assert!(reader.contains("readTimestamp"));
    }

    #[test]
    fn generate_produces_the_expected_tree_for_golden() {
        let catalog = Catalog::from_entities(vec![golden()]).expect("valid catalog");
        let tree = generate(&catalog).expect("generates");
        let paths: Vec<String> = tree
            .files
            .iter()
            .map(|f| f.path.to_string_lossy().into_owned())
            .collect();
        assert!(
            paths
                .iter()
                .any(|p| p.ends_with("er-entities/src/golden_record.rs"))
        );
        assert!(
            paths
                .iter()
                .any(|p| p.ends_with("er-entities/ddl/lake/golden_records.sql"))
        );
        assert!(paths.iter().any(|p| p.ends_with("er-entities/src/lib.rs")));
        // The data-lane golden_record gets both a binding and an Arrow reader.
        assert!(
            paths
                .iter()
                .any(|p| p.ends_with("entities/bindings/GoldenRecord.ts"))
        );
        assert!(
            paths
                .iter()
                .any(|p| p.ends_with("entities/readers/GoldenRecord.ts"))
        );
    }
}
