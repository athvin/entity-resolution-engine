//! Parsing and planning a single statement against the analyser catalog, through
//! feather-flow's `plan_statement` (a pre-parsed AST, so a rewrite step can run
//! first). sqlparser is used only through `ff_analysis::sqlparser` — DataFusion's
//! bundled 0.59 — never the standalone crate.

use datafusion_expr::LogicalPlan;
use ff_analysis::sqlparser::dialect::Dialect;
use ff_analysis::sqlparser::parser::Parser;
use ff_analysis::{FeatherFlowProvider, plan_statement};

/// Parse one statement with the given dialect and plan it against the provider.
/// Errors are returned as strings for now; the typed diagnostics land with the
/// pass framework.
pub fn parse_and_plan(
    sql: &str,
    dialect: &dyn Dialect,
    provider: &FeatherFlowProvider,
) -> Result<LogicalPlan, String> {
    let mut statements =
        Parser::parse_sql(dialect, sql).map_err(|e| format!("parse error: {e}"))?;
    if statements.is_empty() {
        return Err("no statement parsed".to_string());
    }
    let stmt = statements.remove(0);
    plan_statement(stmt, provider).map_err(|e| format!("plan error: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build_store_catalog;
    use er_entity_spec::{Catalog, Store};
    use ff_analysis::FunctionRegistry;
    use ff_analysis::sqlparser::dialect::DuckDbDialect;

    const GOLDEN_YAML: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../entities/golden_record/entity.yaml"
    ));

    fn golden_catalog() -> Catalog {
        Catalog::from_entities(vec![serde_norway::from_str(GOLDEN_YAML).unwrap()]).unwrap()
    }

    /// Plan a lake SQL string against the golden catalog, returning the field count
    /// or the error string. Applies the lake rewrite (strip `AT (VERSION => ..)`)
    /// first, exactly as the analyser will.
    fn plan(sql: &str) -> Result<usize, String> {
        let cat = build_store_catalog(&golden_catalog(), Store::Duckdb);
        let registry = FunctionRegistry::new();
        let provider = FeatherFlowProvider::new(&cat, &registry);
        let dialect = DuckDbDialect {};
        let rewritten = crate::rewrite_lake(sql);
        parse_and_plan(&rewritten.sql, &dialect, &provider).map(|p| p.schema().fields().len())
    }

    #[test]
    fn probe_baseline_select() {
        assert_eq!(
            plan("SELECT entity_id, given_name FROM lake.main.golden_records"),
            Ok(2)
        );
    }

    #[test]
    fn probe_full_projection() {
        let sql = "SELECT entity_id, given_name, family_name, email, phone_e164, addr_number, \
                   addr_street, addr_unit, addr_city, addr_region, addr_postal, birth_date, \
                   metadata, survivorship_version, assembled_at \
                   FROM lake.main.golden_records";
        assert_eq!(plan(sql), Ok(15));
    }

    #[test]
    fn probe_keyset_and_limit_with_placeholders() {
        let sql = "SELECT entity_id FROM lake.main.golden_records \
                   WHERE ($after IS NULL OR entity_id > $after) \
                   ORDER BY entity_id LIMIT $limit";
        let r = plan(sql);
        assert!(r.is_ok(), "keyset+limit did not plan: {r:?}");
    }

    #[test]
    fn probe_concat_coalesce_ilike() {
        let sql = "SELECT entity_id FROM lake.main.golden_records \
                   WHERE ($q IS NULL OR (coalesce(given_name, '') || ' ' || coalesce(family_name, '') \
                   || ' ' || coalesce(email, '')) ILIKE $q ESCAPE '\\')";
        let r = plan(sql);
        assert!(r.is_ok(), "concat/coalesce/ilike did not plan: {r:?}");
    }

    #[test]
    fn at_version_plans_after_rewrite() {
        // sqlparser 0.59 cannot parse `AT (VERSION => ..)`, so the lake rewrite
        // strips it textually before planning. The `plan` helper applies it.
        let sql = "SELECT entity_id, given_name FROM lake.main.golden_records AT (VERSION => $snapshot) \
                   WHERE ($after IS NULL OR entity_id > $after) ORDER BY entity_id LIMIT $limit";
        let r = plan(sql);
        assert_eq!(r, Ok(2), "full lake read did not plan after rewrite: {r:?}");
    }
}
