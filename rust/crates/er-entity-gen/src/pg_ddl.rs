//! The Postgres DDL emitter. Unlike the lake (which carries nothing but NOT
//! NULL), control-plane tables carry the constraints the dispatcher's correctness
//! depends on: primary keys, defaults, references, `accepted_values` as CHECK
//! constraints, and secondary indexes (including partial unique indexes). Rendered
//! as plain text so the migration files read as the SQL they are.

use er_entity_spec::{Entity, Store};

use crate::convert::ConvertError;

/// Render the `CREATE TABLE public.<relation>` statement plus its indexes for a
/// control-plane entity. The result is newline-terminated and ends each statement
/// with `;`.
pub fn emit_postgres_ddl(entity: &Entity) -> Result<String, ConvertError> {
    if !entity.has_store(Store::Postgres) {
        return Err(ConvertError::WrongStore {
            entity: entity.name.clone(),
            store: Store::Postgres,
        });
    }

    let mut lines: Vec<String> = Vec::new();
    for column in &entity.columns {
        // A `serial` bigint renders as `bigserial` (identity) in Postgres DDL only;
        // everywhere else it is a plain bigint.
        let pg_type = if column.serial {
            "bigserial"
        } else {
            column.col_type.postgres_type()
        };
        let mut col = format!("  {} {}", column.name, pg_type);
        if !column.nullable {
            col.push_str(" NOT NULL");
        }
        if let Some(default) = &column.default {
            col.push_str(&format!(" DEFAULT {default}"));
        }
        if let Some(reference) = &column.references {
            col.push_str(&format!(
                " REFERENCES {}({})",
                reference.relation, reference.column
            ));
        }
        if let Some(values) = &column.accepted_values {
            let set = values
                .iter()
                .map(|v| format!("'{}'", v.replace('\'', "''")))
                .collect::<Vec<_>>()
                .join(", ");
            col.push_str(&format!(" CHECK ({} IN ({set}))", column.name));
        }
        lines.push(col);
    }
    // The primary key is the entity's logical key.
    lines.push(format!("  PRIMARY KEY ({})", entity.key.join(", ")));

    let mut out = format!(
        "CREATE TABLE IF NOT EXISTS public.{} (\n{}\n);\n",
        entity.relation,
        lines.join(",\n")
    );

    for index in &entity.indexes {
        let unique = if index.unique { "UNIQUE " } else { "" };
        let cols = index.columns.join(", ");
        let where_clause = index
            .r#where
            .as_ref()
            .map(|w| format!(" WHERE {w}"))
            .unwrap_or_default();
        out.push_str(&format!(
            "CREATE {unique}INDEX IF NOT EXISTS {} ON public.{} ({cols}){where_clause};\n",
            index.name, entity.relation
        ));
    }

    Ok(out)
}
