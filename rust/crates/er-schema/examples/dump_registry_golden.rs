//! Regenerate `tests/model_registry_golden.json` from the live registry:
//!
//! ```sh
//! cargo run -q -p er-schema --example dump_registry_golden \
//!   > crates/er-schema/tests/model_registry_golden.json
//! ```
//!
//! Byte-identical to the file's original recipe (Python
//! `json.dumps(data, indent=1, ensure_ascii=False, sort_keys=True) + '\n'`):
//! sorted keys, one-space indent, trailing newline. The golden stays a pinned,
//! reviewed artifact — this tool exists so a deliberate registry change
//! regenerates it mechanically instead of by hand surgery.

use serde_json::json;
use serde_json::value::Value;

use er_schema::model::{Owner, all_specs, create_table_sql, ddl_owned, stage_owned};

fn main() {
    let mut relations = serde_json::Map::new();
    for spec in all_specs() {
        let columns: Vec<Value> = spec
            .columns
            .iter()
            .map(|c| {
                json!({
                    "name": c.name,
                    "nullable": c.nullable,
                    "type": c.r#type,
                })
            })
            .collect();
        let keys: Vec<Value> = spec
            .keys
            .iter()
            .map(|k| {
                json!({
                    "columns": k.columns,
                    "where": k.r#where,
                })
            })
            .collect();
        let enums: serde_json::Map<String, Value> = spec
            .enums
            .iter()
            .map(|(column, values)| {
                let sorted: Vec<&String> = values.iter().collect();
                (column.clone(), json!(sorted))
            })
            .collect();
        let sql: Value = if spec.owner == Owner::Ddl {
            json!(create_table_sql(&spec).expect("DDL-owned spec renders"))
        } else {
            Value::Null
        };
        relations.insert(
            spec.name.clone(),
            json!({
                "columns": columns,
                "create_table_sql": sql,
                "enums": enums,
                "keys": keys,
                "owner": spec.owner.as_str(),
            }),
        );
    }

    let golden = json!({
        "ddl_owned": ddl_owned(),
        "relations": relations,
        "schema_qualifier": er_schema::model::SCHEMA_QUALIFIER,
        "stage_owned": stage_owned(),
    });

    // Sorted keys come from reserializing through BTreeMap-backed Values; the
    // one-space indent matches Python's `indent=1`.
    let sorted: std::collections::BTreeMap<String, Value> =
        serde_json::from_value(golden).expect("golden reserializes");
    let formatter = serde_json::ser::PrettyFormatter::with_indent(b" ");
    let mut out = Vec::new();
    let mut ser = serde_json::Serializer::with_formatter(&mut out, formatter);
    serde::Serialize::serialize(&sorted, &mut ser).expect("golden serializes");
    out.push(b'\n');
    print!("{}", String::from_utf8(out).expect("golden is UTF-8"));
}
