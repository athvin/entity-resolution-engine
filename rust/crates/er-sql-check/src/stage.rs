//! The lake-stage analyser catalog. The entity statements plan against the
//! entity-owned relations (the golden marts); the engine's *stage* SQL — the
//! mart-populating assembly in particular — also reads the intermediate lake
//! relations the pipeline builds: `int_std_records` (standardized records),
//! `entity_membership` and `entities`. Those are not entities, so they are modeled
//! here from the `er-schema` registry and added to the lake catalog, keyed by the
//! same store-qualified name, so stage SQL plans against one catalog.
//!
//! Coverage. Both the golden-record and the golden-*lineage* mart-assembly SQL
//! plan in full against this catalog. The record assembly needs the DuckDB
//! json/list function stubs in ff-analysis plus the stage rewrites for named
//! arguments and `::json` casts. The lineage assembly additionally builds DuckDB
//! `MAP { 'k': v }` literals and reads them with map-subscript field access
//! (`sources[source_system].priority_rank`): the literals and bare subscripts are
//! planned by ff-analysis's custom `ExprPlanner` (which keeps their inner column
//! references resolved), its `row_number()`/window stubs cover the ranking, and
//! the three-part `m[key].field` access — which DataFusion's dot-handling rejects
//! before the planner sees it — is folded to `element_at(m, key)` by
//! [`crate::rewrite_map_subscript`]. The test below plans both marts.

use std::sync::Arc;

use er_entity_spec::{Catalog, Store};
use ff_analysis::{FloatBitWidth, IntBitWidth};
use ff_analysis::{Nullability, RelSchema, SchemaCatalog, SqlType, TypedColumn};

/// The engine's intermediate lake relations (not entity-owned) that stage SQL
/// reads. The golden marts (`golden_records`, `golden_lineage`, `golden_display`
/// and the account variants) are entities and already come from the catalog.
const STAGE_RELATIONS: [&str; 3] = ["int_std_records", "entity_membership", "entities"];

/// Map an `er-schema` column type string (the eight-type vocabulary's spelling) to
/// the feather-flow `SqlType` subset.
fn schema_type_to_sql(type_str: &str) -> SqlType {
    match type_str {
        "BIGINT" => SqlType::Integer {
            bits: IntBitWidth::I64,
        },
        "DOUBLE" => SqlType::Float {
            bits: FloatBitWidth::F64,
        },
        "BOOLEAN" => SqlType::Boolean,
        "DATE" => SqlType::Date,
        "TIMESTAMP" => SqlType::Timestamp,
        "JSON" => SqlType::Json,
        "LIST(VARCHAR)" => SqlType::Array(Box::new(SqlType::String { max_length: None })),
        // VARCHAR and anything else land on String (the lake's default).
        _ => SqlType::String { max_length: None },
    }
}

/// A `RelSchema` for a registry `TableSpec`: its columns in declaration order.
fn rel_schema(spec: &er_schema::model::TableSpec) -> RelSchema {
    let columns = spec
        .columns
        .iter()
        .map(|c| TypedColumn {
            name: c.name.clone(),
            source_table: Some(spec.name.clone()),
            sql_type: schema_type_to_sql(&c.r#type),
            nullability: if c.nullable {
                Nullability::Nullable
            } else {
                Nullability::NotNull
            },
            provenance: vec![],
        })
        .collect();
    RelSchema::new(columns)
}

/// Build the lake-stage catalog: the DuckDB entity catalog (the golden marts) plus
/// the engine's intermediate lake relations from the registry, so a stage statement
/// that joins marts and intermediate tables plans against one `SchemaCatalog`.
#[must_use]
pub fn build_lake_stage_catalog(entities: &Catalog) -> SchemaCatalog {
    let mut catalog = crate::build_store_catalog(entities, Store::Duckdb);
    for spec in er_schema::model::all_specs() {
        if STAGE_RELATIONS.contains(&spec.name.as_str()) {
            catalog.insert(
                format!("{}.{}", Store::Duckdb.qualifier(), spec.name),
                Arc::new(rel_schema(&spec)),
            );
        }
    }
    catalog
}

#[cfg(test)]
mod tests {
    use super::*;
    use ff_analysis::sqlparser::dialect::DuckDbDialect;
    use ff_analysis::{FeatherFlowProvider, FunctionRegistry};
    use std::path::PathBuf;

    fn entities_catalog() -> Catalog {
        let dir = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../../entities"));
        Catalog::load_dir(dir).expect("entities load")
    }

    const TEST_CONFIG: &str = include_str!("../../er-config/tests/fixtures/test.yaml");

    #[test]
    fn the_stage_catalog_carries_the_intermediate_relations() {
        let cat = build_lake_stage_catalog(&entities_catalog());
        for rel in STAGE_RELATIONS {
            assert!(
                cat.contains_key(&format!("lake.main.{rel}")),
                "stage catalog missing {rel}"
            );
        }
        // …alongside the entity-owned marts.
        assert!(cat.contains_key("lake.main.golden_records"));
        assert!(cat.contains_key("lake.main.golden_lineage"));
    }

    /// Plan a stage statement against the stage catalog, applying the stage
    /// rewrites (snapshot pin, DuckDB named args, ::json casts), returning its
    /// output column names. Panics with the SQL if it does not plan.
    fn plan_stage(catalog: &SchemaCatalog, sql: &str) -> Vec<String> {
        let registry = FunctionRegistry::new();
        let provider = FeatherFlowProvider::new(catalog, &registry);
        let dialect = DuckDbDialect {};
        let rewritten = crate::rewrite_lake(sql);
        let planar = crate::rewrite_map_subscript(&crate::rewrite_json_casts(
            &crate::strip_named_args(&rewritten.sql),
        ));
        let plan = crate::parse_and_plan(&planar, &dialect, &provider)
            .unwrap_or_else(|e| panic!("stage SQL did not plan: {e}\n--- sql ---\n{sql}"));
        plan.schema()
            .fields()
            .iter()
            .map(|f| f.name().clone())
            .collect()
    }

    #[test]
    fn the_golden_record_mart_assembly_sql_plans_against_the_stage_catalog() {
        // The REAL golden_records mart-populating SQL, generated from a
        // representative config, planned against the stage catalog (marts +
        // intermediate tables) through the analyser — proving the DuckDB json/list
        // function stubs resolve and the statement is well-formed against the
        // schema. This is the design's "mart-populating statements under the analyser".
        let config = er_config::load_config_str(TEST_CONFIG).expect("config parses");
        let catalog = build_lake_stage_catalog(&entities_catalog());

        let records = er_golden::golden_records_sql(
            &config,
            "lake.main.golden_lineage",
            "lake.main.int_std_records",
            "lake.main.entity_membership",
            "lake.main.entities",
        );
        let record_cols = plan_stage(&catalog, &records);
        for required in [
            "entity_id",
            "metadata",
            "survivorship_version",
            "assembled_at",
        ] {
            assert!(
                record_cols.contains(&required.to_string()),
                "golden_records output missing {required}: {record_cols:?}"
            );
        }

        // The lineage mart builds DuckDB MAP {} literals + map-subscript field
        // access; the ExprPlanner plans those (keeping the inner column refs
        // resolved), so it too plans against the stage catalog.
        let lineage = er_golden::golden_lineage_sql(
            &config,
            "lake.main.int_std_records",
            "lake.main.entity_membership",
            "lake.main.entities",
        )
        .expect("lineage sql renders");
        let lineage_cols = plan_stage(&catalog, &lineage);
        for required in ["entity_id", "attribute", "record_key", "rule"] {
            assert!(
                lineage_cols.contains(&required.to_string()),
                "golden_lineage output missing {required}: {lineage_cols:?}"
            );
        }
    }
}
