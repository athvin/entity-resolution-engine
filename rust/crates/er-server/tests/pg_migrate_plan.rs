//! The plan-only migrate path against a real Postgres (behind `ER_TEST_PG_DSN`).
//!
//! The unit tests in `er_server::migrate_plan` cover the verdict logic. The
//! property THIS file exists for cannot be unit-tested, because it is about
//! what the database looks like afterwards:
//!
//! > a plan that says "breaking" must leave the store exactly as it found it.
//!
//! That is the whole reason the mode exists, and it is not redundant with the
//! refusal `ensure_schema` already carries. `ensure_schema` is six
//! transactions: phase 1 replays the baseline (`CREATE TABLE IF NOT EXISTS`)
//! and COMMITS, phase 2 applies the `BeforeReconcile` fixups and COMMITS, and
//! only phase 3 reconciles and refuses. So a bare `migrate` against a store
//! that is both behind and breaking *converges part of the way and then halts* —
//! correct for a boot, wrong for a gate that is being asked "may I deploy
//! this?" before anything has been deployed.
//!
//! Measured on the dev stack while writing this, against one store state:
//!
//! | invocation | exit | `webhooks` table afterwards |
//! |---|---|---|
//! | `migrate --expect-additive` | 3 | absent — nothing written |
//! | `migrate` | 1 | **created** — phase 1 committed |
//!
//! The phases are exercised through `pg::ensure_schema` and the planner
//! directly rather than by shelling out to the binary: the test binary IS the
//! library, and spawning `er-server` would need a built artifact on PATH.
//!
//! ONE test with sequential phases (the file-level rule `pg_serial.rs` checks).

use er_entity_spec::Store;
use er_server::migrate_plan::{Verdict, verdict};
use er_server::pg;

use crate::common;

/// The planner over the live store, as `er-server migrate --plan` runs it.
async fn plan_now(client: &tokio_postgres::Client) -> er_entity_diff::MigrationPlan {
    let catalog = er_entities::catalog::catalog();
    let relations = er_entity_diff::postgres_relations(&catalog);
    let live = er_entity_diff::read_postgres_columns(client, &relations)
        .await
        .expect("reading the live schema");
    er_entity_diff::plan_migration(&catalog, Store::Postgres, &live)
}

async fn table_exists(client: &tokio_postgres::Client, name: &str) -> bool {
    client
        .query_one(
            "SELECT EXISTS (SELECT 1 FROM information_schema.tables
             WHERE table_schema = 'public' AND table_name = $1)",
            &[&name],
        )
        .await
        .expect("table existence")
        .get(0)
}

#[tokio::test]
async fn a_breaking_plan_reports_without_touching_the_store() {
    let Some(db) = common::fresh_db("migrateplan").await else {
        return;
    };
    let dsn = db.dsn().to_string();
    let mut client = pg::connect(&dsn).await.unwrap();

    // ---- phase 1: an empty store plans as additive, and stays empty --------
    //
    // Every relation is absent, so every statement is a CREATE — Severity::Safe
    // — and the verdict is the one a first deploy must get.
    let plan = plan_now(&client).await;
    assert_eq!(verdict(&plan), Verdict::Additive);
    assert!(
        !plan.statements.is_empty(),
        "an empty store has statements to apply"
    );
    assert!(plan.breaking.is_empty(), "a fresh store is never breaking");
    let tables: i64 = client
        .query_one(
            "SELECT count(*) FROM information_schema.tables WHERE table_schema = 'public'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(tables, 0, "planning must not create anything");

    // ---- phase 2: applied, it plans as current -----------------------------
    pg::ensure_schema(&mut client).await.unwrap();
    assert_eq!(verdict(&plan_now(&client).await), Verdict::Current);

    // ---- phase 3: a retype is breaking, and an extra column is not ---------
    //
    // These two are the pair the whole blue/green position rests on. A retyped
    // column is refused; a column the catalog does not declare is `Ahead` —
    // tolerated, never dropped, and NOT a reason to refuse, because that is
    // what lets an older image keep serving a store a newer one has migrated.
    client
        .batch_execute("ALTER TABLE public.orgs ADD COLUMN zz_operator_extra text")
        .await
        .unwrap();
    let ahead_plan = plan_now(&client).await;
    assert_eq!(
        verdict(&ahead_plan),
        Verdict::Current,
        "an ahead column must not make a plan unsafe"
    );
    assert_eq!(ahead_plan.ahead.len(), 1, "and it must still be REPORTED");

    client
        .batch_execute("ALTER TABLE public.orgs ALTER COLUMN max_concurrent_jobs TYPE integer")
        .await
        .unwrap();
    let breaking_plan = plan_now(&client).await;
    assert_eq!(verdict(&breaking_plan), Verdict::Breaking);
    assert_eq!(Verdict::Breaking.exit_code(), 3);

    // ---- phase 4: THE POINT — a gate refuses where a boot half-converges ---
    //
    // Drop a table so the store is BOTH behind (phase 1 would recreate it and
    // commit) and breaking (phase 3 refuses on the retype above). A plan must
    // leave the table absent; `ensure_schema` recreates it on its way to
    // failing, which is exactly why a gate cannot be built out of it.
    client
        .batch_execute(
            "DROP TABLE IF EXISTS public.webhook_deliveries CASCADE;\n\
             DROP TABLE IF EXISTS public.webhooks CASCADE;",
        )
        .await
        .unwrap();
    assert!(!table_exists(&client, "webhooks").await);

    let mixed = plan_now(&client).await;
    assert_eq!(verdict(&mixed), Verdict::Breaking);
    assert!(
        !mixed.statements.is_empty(),
        "the plan still lists the safe half it is refusing to apply piecemeal"
    );
    assert!(
        !table_exists(&client, "webhooks").await,
        "planning a breaking release must write NOTHING — this is the property \
         the mode exists for, and the one `ensure_schema`'s own refusal does \
         not have"
    );

    // And the contrast, so the claim above is not just asserted: the boot path
    // DOES write before it refuses.
    let boot = pg::ensure_schema(&mut client).await;
    assert!(boot.is_err(), "the boot still refuses the retype");
    assert!(
        table_exists(&client, "webhooks").await,
        "phase 1 committed the recreate before phase 3 refused — the difference \
         between a boot that converges as far as it safely can and a gate that \
         must not move at all"
    );
}
