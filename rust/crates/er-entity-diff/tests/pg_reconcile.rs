//! The control-plane reconciler over a real Postgres (behind `ER_TEST_PG_DSN`):
//! an empty database is provisioned from the catalog and the ledger says so; a
//! second pass is a no-op; a column dropped behind the reconciler's back comes
//! back as an `applied` row; an extra live column is reported as ahead and left
//! alone; a retyped column is refused and nothing is written. Run with:
//!   ER_TEST_PG_DSN='host=localhost port=5432 user=er password=er dbname=er' \
//!     cargo test -p er-entity-diff --test pg_reconcile
//!
//! ONE test with sequential phases: the Postgres tier shares one schema.

use er_entity_diff::{ReconcileError, ledger_kind, reconcile_postgres};
use er_entity_spec::Store;

#[tokio::test]
async fn the_control_database_converges_and_the_ledger_records_it() {
    let Ok(dsn) = std::env::var("ER_TEST_PG_DSN") else {
        eprintln!("skipping Postgres reconcile test (set ER_TEST_PG_DSN to run)");
        return;
    };
    let (mut client, connection) = tokio_postgres::connect(&dsn, tokio_postgres::NoTls)
        .await
        .unwrap();
    tokio::spawn(connection);
    let catalog = er_entities::catalog::catalog();
    let hash = er_entity_diff::catalog_hash(&catalog, Store::Postgres);

    // ---- phase 1: an empty database is provisioned from the catalog ------
    client
        .batch_execute("DROP SCHEMA public CASCADE; CREATE SCHEMA public;")
        .await
        .unwrap();
    let first = reconcile_postgres(&mut client, &catalog, "pg_reconcile-test")
        .await
        .unwrap();
    assert_eq!(first.catalog_hash, hash);
    assert!(!first.ledger_was_current);
    let relations = er_entity_diff::postgres_relations(&catalog);
    // Every declared relation was created (one CREATE per entity, plus the
    // history prelude and nothing for companions, which ride their entity's DDL).
    let created: std::collections::BTreeSet<&str> =
        first.applied.iter().map(|p| p.relation.as_str()).collect();
    for rel in &relations {
        if rel.ends_with("_history") {
            continue;
        }
        assert!(
            created.contains(format!("public.{rel}").as_str()),
            "{rel} was not created: {created:?}"
        );
    }
    let rows = client
        .query(
            "SELECT kind, relation, catalog_hash FROM public.schema_migrations ORDER BY id",
            &[],
        )
        .await
        .unwrap();
    assert_eq!(
        rows.len(),
        first.applied.len(),
        "one ledger row per statement"
    );
    assert!(
        rows.iter()
            .all(|r| r.get::<_, String>(0) == ledger_kind(true) && r.get::<_, String>(2) == hash)
    );

    // ---- phase 2: a second pass is a no-op and writes nothing --------------
    let second = reconcile_postgres(&mut client, &catalog, "pg_reconcile-test")
        .await
        .unwrap();
    assert!(second.applied.is_empty());
    assert!(second.ledger_was_current);
    let count: i64 = client
        .query_one("SELECT count(*) FROM public.schema_migrations", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(
        count,
        rows.len() as i64,
        "a current store writes no ledger row"
    );

    // ---- phase 3: a dropped nullable column comes back -------------------
    client
        .batch_execute("ALTER TABLE public.jobs DROP COLUMN trace_context")
        .await
        .unwrap();
    let third = reconcile_postgres(&mut client, &catalog, "pg_reconcile-test")
        .await
        .unwrap();
    assert_eq!(third.applied.len(), 1, "{:?}", third.applied);
    assert_eq!(third.applied[0].relation, "public.jobs");
    assert!(
        third.applied[0]
            .sql
            .contains("ADD COLUMN trace_context text")
    );
    let back: i64 = client
        .query_one(
            "SELECT count(*) FROM information_schema.columns \
             WHERE table_schema = 'public' AND table_name = 'jobs' AND column_name = 'trace_context'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(back, 1);
    let last = client
        .query_one(
            "SELECT kind, relation, statement FROM public.schema_migrations ORDER BY id DESC LIMIT 1",
            &[],
        )
        .await
        .unwrap();
    assert_eq!(last.get::<_, String>(0), "applied");
    assert_eq!(last.get::<_, String>(1), "public.jobs");
    assert!(last.get::<_, String>(2).contains("trace_context"));

    // ---- phase 4: an extra live column is ahead, tolerated, reported -------
    client
        .batch_execute("ALTER TABLE public.orgs ADD COLUMN legacy_note text")
        .await
        .unwrap();
    let fourth = reconcile_postgres(&mut client, &catalog, "pg_reconcile-test")
        .await
        .unwrap();
    assert!(fourth.applied.is_empty());
    assert_eq!(fourth.ahead.len(), 1, "{:?}", fourth.ahead);
    assert_eq!(fourth.ahead[0].relation, "public.orgs");
    assert!(fourth.ahead[0].message.contains("legacy_note"));
    assert_eq!(fourth.status(), "current");
    client
        .batch_execute("ALTER TABLE public.orgs DROP COLUMN legacy_note")
        .await
        .unwrap();

    // ---- phase 5: a retyped column is refused and nothing is written -------
    client
        .batch_execute("ALTER TABLE public.jobs ALTER COLUMN priority TYPE integer")
        .await
        .unwrap();
    let before: i64 = client
        .query_one("SELECT count(*) FROM public.schema_migrations", &[])
        .await
        .unwrap()
        .get(0);
    let err = reconcile_postgres(&mut client, &catalog, "pg_reconcile-test")
        .await
        .unwrap_err();
    match &err {
        ReconcileError::Breaking(items) => {
            assert!(
                items
                    .iter()
                    .any(|d| d.relation == "public.jobs" && d.message.contains("priority")),
                "{items:?}"
            );
        }
        other => panic!("expected a breaking refusal, got {other}"),
    }
    assert!(err.to_string().starts_with("ERR_SCHEMA_BREAKING"));
    let after: i64 = client
        .query_one("SELECT count(*) FROM public.schema_migrations", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(before, after, "a refusal writes no ledger row");
    client
        .batch_execute("ALTER TABLE public.jobs ALTER COLUMN priority TYPE bigint")
        .await
        .unwrap();
}
