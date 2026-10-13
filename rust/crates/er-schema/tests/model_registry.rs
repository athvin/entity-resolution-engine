//! The S5 registry transcription, validated against a golden dump from the real
//! Python `er.lake.model` (vendored `tests/model_registry_golden.json`): exact
//! `create_table_sql` + owner/columns/keys/enums for every relation present.

use std::collections::BTreeSet;

use er_schema::model::{Owner, all_specs, create_table_sql, dbt_owned, ddl_owned};

#[test]
fn registry_matches_python_golden() {
    let raw = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/model_registry_golden.json"
    ));
    let golden: serde_json::Value = serde_json::from_str(raw).unwrap();

    for spec in all_specs() {
        let g = &golden["relations"][spec.name.as_str()];
        assert!(!g.is_null(), "{} not in golden", spec.name);

        assert_eq!(
            spec.owner.as_str(),
            g["owner"].as_str().unwrap(),
            "{} owner",
            spec.name
        );

        if spec.owner == Owner::Ddl {
            assert_eq!(
                create_table_sql(&spec).unwrap(),
                g["create_table_sql"].as_str().unwrap(),
                "{} create_table_sql",
                spec.name
            );
        }

        let gcols = g["columns"].as_array().unwrap();
        assert_eq!(
            spec.columns.len(),
            gcols.len(),
            "{} column count",
            spec.name
        );
        for (column, gc) in spec.columns.iter().zip(gcols) {
            assert_eq!(
                column.name,
                gc["name"].as_str().unwrap(),
                "{} column name",
                spec.name
            );
            assert_eq!(
                column.r#type,
                gc["type"].as_str().unwrap(),
                "{} column type",
                spec.name
            );
            assert_eq!(
                column.nullable,
                gc["nullable"].as_bool().unwrap(),
                "{} nullable",
                spec.name
            );
        }

        let gkeys = g["keys"].as_array().unwrap();
        assert_eq!(spec.keys.len(), gkeys.len(), "{} key count", spec.name);
        for (k, gk) in spec.keys.iter().zip(gkeys) {
            let gk_cols: Vec<&str> = gk["columns"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap())
                .collect();
            assert_eq!(k.columns, gk_cols, "{} key columns", spec.name);
            assert_eq!(
                k.r#where.as_deref(),
                gk["where"].as_str(),
                "{} key where",
                spec.name
            );
        }

        for (column, values) in &spec.enums {
            let gvals: BTreeSet<&str> = g["enums"][column]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap())
                .collect();
            let mine: BTreeSet<&str> = values.iter().map(String::as_str).collect();
            assert_eq!(mine, gvals, "{} enum {}", spec.name, column);
        }
    }

    // DDL- and DBT-owned names, in declaration order, match the golden.
    let golden_ddl: Vec<&str> = golden["ddl_owned"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(ddl_owned(), golden_ddl);
    let golden_dbt: Vec<&str> = golden["dbt_owned"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(dbt_owned(), golden_dbt);

    // All 22 relations in the golden are present in the registry.
    assert_eq!(
        all_specs().len(),
        golden["relations"].as_object().unwrap().len()
    );
}
