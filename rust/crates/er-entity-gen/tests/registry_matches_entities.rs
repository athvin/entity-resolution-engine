//! The entity catalog and the hand-maintained S5 registry must describe the
//! same lake tables.
//!
//! There are TWO schema sources, which is easy to miss: `entities/*/entity.yaml`
//! (generated into `er-entities/ddl/lake/*.sql`, the entity framework's output)
//! and `er_schema::model::all_specs()` — a hand-maintained registry pinned by
//! its own golden dump. They are not the same code path, and the one that runs
//! at RUNTIME is the registry: `er_cli::create_ddl_table` builds every lake
//! table from `all_specs()`, not from the generated DDL.
//!
//! So a column added to an `entity.yaml` and regenerated is invisible to any
//! lake table created at run time until the registry is updated too. That is
//! not hypothetical: E9b added four columns to `ingest_batch`, regenerated
//! cleanly, passed `er-entity verify` AND `er-entity-diff`, and then failed in
//! a lake test with `Binder Error: Referenced column "matched_count" not
//! found` — because the table had been created from a registry that had never
//! heard of it.
//!
//! `er-entity-diff` cannot catch this by construction: it compares the catalog
//! against a LIVE store, so with no live store carrying the new column there is
//! nothing to diff. This test is the static half.
//!
//! The comparison is `create_table_sql` on both sides — one string covering
//! names, types, nullability and declaration order, rendered by the same
//! function, so a difference anywhere in the shape fails with a readable diff.

use er_entity_gen::to_lake_table_spec;
use er_entity_spec::{Catalog, Store};
use er_schema::model::{Owner, all_specs, create_table_sql};

#[test]
fn every_entity_lake_table_matches_the_s5_registry() {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../entities");
    let catalog = Catalog::load_dir(&root).expect("the entity catalog loads");
    let specs = all_specs();

    let mut compared: Vec<&str> = Vec::new();
    let mut missing: Vec<&str> = Vec::new();
    for entity in catalog.entities() {
        if !entity.has_store(Store::Duckdb) {
            continue;
        }
        let Some(registry) = specs.iter().find(|s| s.name == entity.relation) else {
            // ABSENT from the registry, which this test used to skip on the
            // reasoning that "an entity may be declared before its registry
            // entry exists". That reasoning cost a debugging cycle: a DuckDB
            // entity the registry has never heard of can never be created,
            // because `create_ddl_table` builds from the REGISTRY — and the one
            // caller that matters (`write_metric_snapshot`) is best-effort, so
            // the failure arrives as a swallowed log line reading
            // "<relation> spec missing from S5 registry" and the table simply
            // never appears.
            missing.push(entity.relation.as_str());
            continue;
        };
        // Stage-owned relations have no DDL to compare: the engine materialises
        // them with CTAS, and `create_table_sql` refuses them outright. Their
        // shape is pinned by the stage SQL and its goldens instead.
        if registry.owner != Owner::Ddl {
            continue;
        }
        let from_entity = to_lake_table_spec(entity).expect("entity converts to a table spec");
        compared.push(entity.relation.as_str());
        assert_eq!(
            create_table_sql(&from_entity).expect("entity-derived DDL renders"),
            create_table_sql(registry).expect("registry DDL renders"),
            "`{}`: the entity catalog and the S5 registry disagree. Both must be \
             updated for a lake column change — `create_ddl_table` builds from the \
             REGISTRY at run time, so an entity-only change is invisible to every \
             table created by a run.",
            entity.relation
        );
    }

    assert!(
        missing.is_empty(),
        "these DuckDB entities have no S5 registry spec, so `create_ddl_table` \
         cannot build their tables and every write to them fails at run time: \
         {missing:?}. Add a `tspec` to `er_schema::model::all_specs()`."
    );

    // A guard on the guard: every `continue` above is a silent skip, so a
    // catalog that stopped resolving — or an `owner` vocabulary change — would
    // make this test pass having compared nothing. Naming a relation rather
    // than asserting a count keeps it honest without going stale every time an
    // entity is added: `ingest_batches` is DDL-owned, in both sources, and the
    // one whose drift motivated this gate.
    assert!(
        compared.contains(&"ingest_batches"),
        "ingest_batches must be compared — it is DDL-owned and in both sources; \
         compared {compared:?}"
    );
    assert!(
        compared.len() >= 12,
        "a floor, not an equality: the overlap grows as entities are added, but it \
         shrinking means relations silently stopped being checked. Compared {}: {compared:?}",
        compared.len()
    );
}
