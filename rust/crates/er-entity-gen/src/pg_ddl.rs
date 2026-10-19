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
    // The primary key is the entity's logical key; the natural key (what makes
    // two rows the same thing — the upsert identity) renders as UNIQUE.
    lines.push(format!("  PRIMARY KEY ({})", entity.key.join(", ")));
    if !entity.natural_key.is_empty() {
        lines.push(format!("  UNIQUE ({})", entity.natural_key.join(", ")));
    }

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

    // The history companion + trigger ride in the entity's own DDL entry.
    if let Some(companion) = emit_history_companion_ddl(entity) {
        out.push_str(&companion);
    }

    Ok(out)
}

/// The history companion + trigger for a `history: true` entity: a generated
/// jsonb row-image ledger, not an entity. The trigger routes through the one
/// `public.er_history()` function (the baseline prelude) with the companion
/// name as its argument. `None` for entities without the flag.
#[must_use]
pub fn emit_history_companion_ddl(entity: &Entity) -> Option<String> {
    if !entity.generate.history {
        return None;
    }
    let mut out = String::new();
    {
        let relation = &entity.relation;
        out.push_str(&format!(
            "CREATE TABLE IF NOT EXISTS public.{relation}_history (\n  \
             history_id bigserial NOT NULL,\n  \
             row_id text NOT NULL,\n  \
             op text NOT NULL CHECK (op IN ('insert', 'update', 'delete')),\n  \
             changed_at timestamptz NOT NULL DEFAULT now(),\n  \
             actor text NOT NULL,\n  \
             old_row jsonb,\n  \
             new_row jsonb,\n  \
             PRIMARY KEY (history_id)\n);\n\
             CREATE INDEX IF NOT EXISTS {relation}_history_row_id ON public.{relation}_history (row_id);\n\
             CREATE INDEX IF NOT EXISTS {relation}_history_changed_at ON public.{relation}_history (changed_at);\n\
             CREATE OR REPLACE TRIGGER er_history_{relation}\n  \
             AFTER INSERT OR UPDATE OR DELETE ON public.{relation}\n  \
             FOR EACH ROW EXECUTE FUNCTION public.er_history('{relation}_history');\n"
        ));
    }
    Some(out)
}
