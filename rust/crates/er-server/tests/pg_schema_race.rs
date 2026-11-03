//! Two processes ensuring the control-plane schema at once, against a LIVE
//! OLDER store (behind `ER_TEST_PG_DSN`).
//!
//! `ensure_schema`'s module doc says the advisory lock exists because "the
//! drop-then-recreate index idiom deadlocks without serialization". It is right,
//! and for a while one phase did not take it. `ensure_schema` is six
//! transactions, not one, releasing the xact lock between each:
//!
//! | phase | work | advisory lock |
//! |---|---|---|
//! | 1 | baseline replay, deferring `undefined_column` statements | yes |
//! | 2 | `BeforeReconcile` fixups | yes, per fixup |
//! | 3 | the reconciler's ADD COLUMNs | yes |
//! | 4 | **the deferred baseline replay — `CREATE [UNIQUE] INDEX`** | **no** |
//! | 5 | `AfterReconcile` fixups — `0003_per_dataset_locks`, which `DROP INDEX`es | yes |
//! | 6 | the default-dataset convergence sweep | yes |
//!
//! Phase 4 is the `CREATE INDEX` half of exactly the idiom the module doc
//! names, and it was the one phase running unserialised — so two processes
//! could execute catalog DDL at the same time, which every other phase makes
//! impossible. The observed failure, from running this against the unserialised
//! version:
//!
//! ```text
//! ERROR: deadlock detected
//! DETAIL: Process A waits for ShareRowExclusiveLock on relation R; blocked by process B.
//!         Process B waits for ShareRowExclusiveLock on relation R; blocked by process A.
//! ```
//!
//! A mutual lock *upgrade*, not the drop-versus-create cycle that phase 4's own
//! comment ("so the new-named locks exist before the old-named ones are
//! dropped") might suggest: each process already holds a weaker lock on R from
//! an earlier statement in its transaction, and both then need
//! ShareRowExclusiveLock on it. That level comes from the trigger and
//! foreign-key statements among the deferred set, not from the indexes. The
//! ordering that comment promises is only guaranteed WITHIN one process, which
//! is the whole point.
//!
//! **Two concurrent boots are not enough to see it.** Phases 1-3 all take the
//! lock, so a pair serialises through them and stays naturally offset by the
//! time either reaches phase 4. Six boots overlap enough to collide, and that
//! is why this survived as long as it did. Measured on the unserialised
//! version: six boots failed on round 0 in 3 runs out of 3, while the
//! serialised version passed 4 runs out of 4.
//!
//! The processes that really do this concurrently in a deploy: the `er-migrate`
//! init container, the API's own boot, the standalone `er-dispatcher`'s boot
//! (whose comment says "advisory-locked, so it's safe alongside the API's own
//! ensure_schema"), and a hand-run `er-server migrate`. `Recreate` at one
//! replica narrows the window; it does not close it, and anything that raises
//! the number of simultaneous booters widens it again.
//!
//! A fresh database cannot reproduce this: there the baseline creates every
//! table already carrying its full column set, so phase 4 is empty and phase 5
//! has nothing to drop. The store has to be AGED first, which is why this
//! borrows `pg_schema_replay.rs`'s recipe.
//!
//! Run with the DSN of a superuser on an admin database — note this is NOT the
//! `user=er` spelling `pg_schema_replay.rs` and the Makefile print, which does
//! not match what `frontend/dev/compose.yaml` actually starts (it sets only
//! `POSTGRES_PASSWORD`, so the role and database are both `postgres`):
//!   ER_TEST_PG_DSN='host=localhost port=5433 user=postgres password=er dbname=postgres' \
//!     cargo test -p er-server --test main pg_schema_race
//!
//! ONE test with sequential phases (the file-level rule `pg_serial.rs` checks).

use er_server::pg;

use crate::common;

/// Each round re-ages the store, because a healed one has nothing deferred and
/// no fixup left to apply — the second round of a naive loop would prove
/// nothing. Rounds beyond the first are margin: the unserialised version failed
/// on round 0 every time.
const ROUNDS: usize = 6;

/// Boots per round. **Two is not enough** — see the module header. Phases 1-3
/// serialise a pair into staying offset, so the collision needs more
/// simultaneous booters than any single deploy creates today; this is the
/// number at which it became reliable rather than occasional.
const BOOTERS: usize = 6;

/// Age the store back to the pre-dataset shape: drop every `dataset_id`, the
/// `datasets` table, and the fixup ledger, so the next boot has deferred
/// baseline statements (phase 4) AND an unapplied `0003_per_dataset_locks`
/// (phase 5) — the two phases whose ordering is the whole question.
async fn age(client: &tokio_postgres::Client) -> usize {
    let tables: Vec<String> = client
        .query(
            "SELECT table_name FROM information_schema.columns
             WHERE table_schema = 'public' AND column_name = 'dataset_id'",
            &[],
        )
        .await
        .unwrap()
        .iter()
        .map(|r| r.get::<_, String>(0))
        .collect();
    for table in &tables {
        client
            .batch_execute(&format!(
                "ALTER TABLE public.\"{table}\" DROP COLUMN dataset_id CASCADE"
            ))
            .await
            .unwrap();
    }
    client
        .batch_execute("DROP TABLE IF EXISTS public.datasets CASCADE")
        .await
        .unwrap();
    client
        .batch_execute("DELETE FROM public.fixups")
        .await
        .unwrap();
    tables.len()
}

/// One process's boot: its own connection, as a separate pod would have.
async fn boot(dsn: &str) -> Result<(), String> {
    let mut client = pg::connect(dsn).await.map_err(|e| e.to_string())?;
    pg::ensure_schema(&mut client)
        .await
        .map_err(|e| er_server::errfmt::chain(&e))
}

#[tokio::test]
async fn concurrent_boots_against_an_aged_store_do_not_deadlock() {
    let Some(db) = common::fresh_db("schemarace").await else {
        return;
    };
    let dsn = db.dsn().to_string();

    // ---- phase 1: a current store, then age it ----------------------------
    let mut client = pg::connect(&dsn).await.unwrap();
    pg::ensure_schema(&mut client).await.unwrap();
    let aged = age(&client).await;
    assert!(
        aged >= 10,
        "the catalog should scope well over ten tables by dataset, got {aged}"
    );

    // ---- phase 2: many boots at once, repeatedly --------------------------
    //
    // A deadlock victim is a real boot failure, not a warning: its
    // `ensure_schema` returns Err and `main.rs` turns that into a non-zero
    // exit — so in the cluster it is an init container that fails and a pod
    // that never starts.
    for round in 0..ROUNDS {
        if round > 0 {
            age(&client).await;
        }
        let mut booters = Vec::new();
        for _ in 0..BOOTERS {
            let dsn = dsn.clone();
            booters.push(tokio::spawn(async move { boot(&dsn).await }));
        }
        for (nth, booter) in booters.into_iter().enumerate() {
            let outcome = booter.await.expect("the boot task itself must not panic");
            assert!(
                outcome.is_ok(),
                "round {round}: concurrent boot {nth} failed: {}",
                outcome.unwrap_err()
            );
        }
    }

    // ---- phase 3: and the store is actually current afterwards ------------
    //
    // Every boot returning Ok is not enough on its own. Serialising phase 4
    // also has to leave the schema in the state one boot alone would have
    // produced, whichever process got where first — so check the lock swap
    // landed rather than trusting the absence of an error.
    let healed: i64 = client
        .query_one(
            "SELECT count(*) FROM information_schema.columns
             WHERE table_schema = 'public' AND column_name = 'dataset_id'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(
        healed as usize, aged,
        "every aged table regained its dataset_id column"
    );
    let per_dataset: bool = client
        .query_one(
            "SELECT EXISTS (SELECT 1 FROM pg_indexes
             WHERE schemaname = 'public' AND indexname = 'jobs_one_active_per_dataset')",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert!(
        per_dataset,
        "the per-dataset writer lock must exist after concurrent boots — this \
         index is created by a PHASE 4 statement, so a deadlock victim rolling \
         that transaction back is how the queue silently loses its \
         one-active-job-per-dataset guarantee"
    );
    let legacy: bool = client
        .query_one(
            "SELECT EXISTS (SELECT 1 FROM pg_indexes
             WHERE schemaname = 'public' AND indexname = 'jobs_one_active_per_org')",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert!(
        !legacy,
        "the superseded org-level lock must be gone — a stale one silently \
         keeps the whole org at one job, which is the limit datasets lifted"
    );
}
