//! The DDL ledger (behind `ER_TEST_PG_DSN`, superuser — CI/dev): the
//! `er_ddl_audit_*` event triggers capture migrations and operator DDL into
//! `public.ddl_log`, boot replays are suppressed by the skip GUC, and the
//! declared actor lands on captured rows. Run with e.g.
//!   ER_TEST_PG_DSN='host=localhost port=5455 user=er password=er dbname=er' \
//!     cargo test -p er-server --test pg_ddl_log

use er_server::pg;

async fn ledger_count(client: &tokio_postgres::Client) -> i64 {
    client
        .query_one("SELECT count(*) FROM public.ddl_log", &[])
        .await
        .expect("count ddl_log")
        .get(0)
}

#[tokio::test]
async fn the_ddl_ledger_captures_operator_ddl_but_not_boot_replays() {
    let Ok(dsn) = std::env::var("ER_TEST_PG_DSN") else {
        eprintln!("skipping DDL-ledger test (set ER_TEST_PG_DSN to run)");
        return;
    };

    let mut client = pg::connect(&dsn).await.expect("connect");
    pg::ensure_schema(&mut client).await.expect("ensure schema");
    let installed = pg::ensure_event_triggers(&mut client)
        .await
        .expect("ensure event triggers");
    assert!(
        installed,
        "the test role is a superuser; install must succeed"
    );

    // Boot replays are suppressed: a second ensure_schema adds no ledger rows.
    let before = ledger_count(&client).await;
    pg::ensure_schema(&mut client).await.expect("re-ensure");
    assert_eq!(
        ledger_count(&client).await,
        before,
        "the baseline replay must not spam the ledger (er.ddl_log_skip)"
    );

    // Operator DDL IS captured, with the declared actor.
    pg::set_actor(&client, "ddl-test@example")
        .await
        .expect("set actor");
    client
        .batch_execute("CREATE TABLE IF NOT EXISTS public.zz_ddl_probe (x text)")
        .await
        .expect("probe create");
    client
        .batch_execute("ALTER TABLE public.zz_ddl_probe ADD COLUMN y text")
        .await
        .expect("probe alter");
    client
        .batch_execute("DROP TABLE public.zz_ddl_probe")
        .await
        .expect("probe drop");

    let rows = client
        .query(
            "SELECT phase, ddl_tag, object_identity, actor FROM public.ddl_log \
             WHERE object_identity LIKE '%zz_ddl_probe%' ORDER BY id",
            &[],
        )
        .await
        .expect("read ledger");
    let shapes: Vec<(String, String, String)> = rows
        .iter()
        .map(|r| {
            (
                r.get::<_, String>(0),
                r.get::<_, String>(1),
                r.get::<_, String>(3),
            )
        })
        .collect();
    assert!(
        shapes.iter().any(|(p, t, a)| p == "ddl_command_end"
            && t == "CREATE TABLE"
            && a == "ddl-test@example"),
        "CREATE captured with the declared actor: {shapes:?}"
    );
    assert!(
        shapes
            .iter()
            .any(|(p, t, _)| p == "ddl_command_end" && t == "ALTER TABLE"),
        "ALTER captured: {shapes:?}"
    );
    assert!(
        shapes
            .iter()
            .any(|(p, t, _)| p == "sql_drop" && t == "DROP TABLE"),
        "DROP captured by the sql_drop phase: {shapes:?}"
    );
    // ddl_command_end skips DROP tags — the drop appears exactly once.
    assert_eq!(
        shapes.iter().filter(|(_, t, _)| t == "DROP TABLE").count(),
        1,
        "no double-logging of drops: {shapes:?}"
    );

    // A fresh connection without the actor GUC records the session user.
    let other = pg::connect(&dsn).await.expect("second connect");
    other
        .batch_execute("CREATE TABLE IF NOT EXISTS public.zz_ddl_probe2 (x text); DROP TABLE public.zz_ddl_probe2;")
        .await
        .expect("anonymous ddl");
    let actor: String = client
        .query_one(
            "SELECT actor FROM public.ddl_log WHERE object_identity LIKE '%zz_ddl_probe2%' \
             ORDER BY id LIMIT 1",
            &[],
        )
        .await
        .expect("anonymous row")
        .get(0);
    assert!(
        !actor.is_empty() && actor != "ddl-test@example",
        "falls back to session user: {actor}"
    );

    // Idempotency: re-installing the event triggers is a no-op.
    assert!(
        pg::ensure_event_triggers(&mut client)
            .await
            .expect("re-install")
    );

    // Cleanup the probe rows so repeated local runs stay tidy.
    let _ = client
        .execute(
            "DELETE FROM public.ddl_log WHERE object_identity LIKE '%zz_ddl_probe%'",
            &[],
        )
        .await;
}
