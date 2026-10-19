//! The posterity gate, control-plane half (behind `ER_TEST_PG_DSN`): after boot
//! provisioning, `public.*` holds ONLY the tables the generated baseline declares
//! — i.e. every control-plane table came from an `entities/*/entity.yaml` through
//! `er-entity-gen`, and nothing hand-created rides along. Run with e.g.
//!   ER_TEST_PG_DSN='host=localhost port=5433 user=er password=er dbname=er' \
//!     cargo test -p er-server --test pg_schema_coverage

use std::collections::BTreeSet;

use er_server::pg;

/// The table names the generated baseline creates, parsed from its own DDL.
/// Every occurrence per entry counts — a baseline entry may carry companion
/// tables (e.g. a `<relation>_history`) beside the entity's own.
fn baseline_tables() -> BTreeSet<String> {
    er_entities::schema::POSTGRES_BASELINE
        .iter()
        .flat_map(|ddl| ddl.split("CREATE TABLE IF NOT EXISTS public.").skip(1))
        .map(|rest| {
            let end = rest
                .find(|c: char| !(c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'))
                .unwrap_or(rest.len());
            rest[..end].to_string()
        })
        .collect()
}

#[tokio::test]
async fn public_schema_contains_only_declared_tables() {
    let Ok(dsn) = std::env::var("ER_TEST_PG_DSN") else {
        eprintln!("skipping Postgres schema-coverage gate (set ER_TEST_PG_DSN to run)");
        return;
    };

    let mut client = pg::connect(&dsn).await.unwrap();
    pg::ensure_schema(&mut client).await.unwrap();

    let declared = baseline_tables();
    assert!(
        declared.contains("orgs") && declared.contains("jobs"),
        "baseline parse looks wrong: {declared:?}"
    );

    let live = client
        .query(
            "SELECT tablename FROM pg_tables WHERE schemaname = 'public'",
            &[],
        )
        .await
        .unwrap();
    let undeclared: Vec<String> = live
        .iter()
        .map(|row| row.get::<_, String>(0))
        .filter(|name| !declared.contains(name))
        .collect();
    assert!(
        undeclared.is_empty(),
        "public.* tables that no entity declares (add an entities/*/entity.yaml \
         and run er-entity gen instead of hand DDL): {undeclared:?}"
    );
}
