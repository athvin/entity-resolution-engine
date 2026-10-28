//! One Postgres database per test, so the `pg_*` tests stop sharing a schema.
//!
//! Every `pg_*.rs` used to connect to the single database named by
//! `ER_TEST_PG_DSN`, `DROP TABLE ... CASCADE` its way to a clean slate, and
//! rebuild. That had two costs.
//!
//! The first was correctness, and `pg_serial.rs` existed to contain it: two
//! tests in one file are two tests racing on the same tables, and the failure
//! looks like a product bug rather than a test-isolation one (`pg_segments.rs`
//! asserted a refused config never activated, read the *other* test's
//! deliberately-valid one, and reported `left: 200, right: 422` against a
//! publish gate that was working perfectly).
//!
//! The second was time. Because every file had to be a single test, and cargo
//! runs one test binary at a time, the 41 `pg_*` binaries ran strictly in
//! series — 170s of the `DuckLake + extensions e2e` job spent waiting rather
//! than working.
//!
//! A database per test removes both at the source. It also removes the
//! hand-maintained drop list, which had already drifted into **ten** distinct
//! variants across 35 files — a list that is wrong in the silent direction,
//! since a table missing from it survives into the next test.
//!
//! The database is created EMPTY and each test still calls
//! `pg::ensure_schema` itself. That is deliberate: schema bootstrap is part of
//! what several of these tests exercise, so cloning a pre-built template would
//! quietly stop testing it. If `ensure_schema` ever dominates the runtime,
//! `CREATE DATABASE ... TEMPLATE` is the next step — but it is an optimisation,
//! not the point.

#![allow(dead_code)] // Each test binary uses a subset of this module.

use er_server::pg;

/// A Postgres database owned by exactly one test.
///
/// Not dropped on the way out, deliberately: the name is derived from the
/// test's slug rather than randomised, so a failed run leaves its database
/// behind to inspect and the next run reclaims it. That bounds the count at one
/// per test instead of one per run.
pub struct TestDb {
    name: String,
    dsn: String,
}

impl TestDb {
    /// The DSN for this test's own database. Pass it to both `pg::connect` and
    /// `AppState::with_pg` — they must agree, or the handler under test and the
    /// assertions read different databases.
    pub fn dsn(&self) -> &str {
        &self.dsn
    }

    pub fn name(&self) -> &str {
        &self.name
    }
}

/// Create a fresh, empty database for one test, or `None` when
/// `ER_TEST_PG_DSN` is unset.
///
/// `None` preserves the established skip: these tests are gated on a real
/// Postgres and print why when they do not run. Callers must keep returning
/// early on `None` rather than unwrapping, or the `test` job (which has no
/// Postgres) goes red.
///
/// `slug` names the database and so must be a bare lowercase identifier.
/// Conventionally the test file's subject — `"tags"` for `pg_tags.rs`.
///
/// One slug per *file*, and calling it twice in one process is a bug rather
/// than a supported pattern: the second call would `DROP DATABASE ... WITH
/// (FORCE)` the first caller's database out from under it. `pg_serial.rs`
/// enforces the file-level half of that rule; the assertion below catches the
/// case it cannot see.
pub async fn fresh_db(slug: &str) -> Option<TestDb> {
    let Ok(admin_dsn) = std::env::var("ER_TEST_PG_DSN") else {
        eprintln!("skipping Postgres test (set ER_TEST_PG_DSN to run)");
        return None;
    };

    assert!(
        !slug.is_empty()
            && slug.len() <= 40
            && slug
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
        "test database slug {slug:?} must be a bare lowercase identifier: it is \
         interpolated into CREATE DATABASE, which takes no parameters"
    );
    let name = format!("er_t_{slug}");

    // Two calls in one process would fight over one database, and the loser's
    // connection is simply closed mid-query — a failure that reads like a
    // flaky Postgres rather than a test-authoring mistake.
    static CLAIMED: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
    {
        let mut claimed = CLAIMED.lock().expect("the claim list is not poisoned");
        assert!(
            !claimed.contains(&name),
            "fresh_db({slug:?}) was called twice in one test binary. The slug names the \
             database, so the second call drops the first caller's. Give each test its own \
             slug, or make the extra ones sequential phases of one test (see pg_serial.rs)."
        );
        claimed.push(name.clone());
    }

    let admin = pg::connect(&admin_dsn)
        .await
        .expect("connecting to the admin database named by ER_TEST_PG_DSN");

    // WITH (FORCE) terminates anything still attached from an earlier aborted
    // run; without it a leaked connection makes the DROP hang rather than fail.
    // Postgres 13+, and CI pins postgres:16-alpine.
    admin
        .batch_execute(&format!("DROP DATABASE IF EXISTS {name} WITH (FORCE);"))
        .await
        .unwrap_or_else(|e| panic!("dropping the previous {name}: {e}"));

    // Concurrent CREATE DATABASE calls copy `template1`, and Postgres refuses
    // while another session is connected to the source. With tests running in
    // parallel that is a routine collision, not an error worth failing on.
    let mut attempt = 0;
    loop {
        match admin
            .batch_execute(&format!("CREATE DATABASE {name};"))
            .await
        {
            Ok(()) => break,
            Err(e) if attempt < 10 && e.to_string().contains("being accessed by other users") => {
                attempt += 1;
                tokio::time::sleep(std::time::Duration::from_millis(100 * attempt)).await;
            }
            Err(e) => panic!("creating {name}: {e}"),
        }
    }

    Some(TestDb {
        dsn: with_dbname(&admin_dsn, &name),
        name,
    })
}

/// Repoint a libpq keyword/value DSN at another database.
///
/// Only the keyword/value form is handled, which is the form every `pg_*.rs`
/// doc comment, `CONTRIBUTING.md` and `.github/workflows/rust.yml` use. A URL
/// DSN would need different surgery, so it fails loudly instead of silently
/// connecting to the wrong database.
///
/// Public, and asserted from `db_per_test.rs` rather than from a `#[cfg(test)]`
/// module here: this file is `mod common;`-ed into every `pg_*.rs` binary, so a
/// test living in it would run once per binary and report ~38 duplicates.
pub fn with_dbname(dsn: &str, dbname: &str) -> String {
    assert!(
        !dsn.contains("://"),
        "ER_TEST_PG_DSN must be in libpq keyword/value form (`host=... dbname=...`), \
         not a URL: the per-test database is selected by rewriting `dbname=`"
    );
    let mut out: Vec<String> = dsn
        .split_whitespace()
        .filter(|kv| !kv.starts_with("dbname="))
        .map(str::to_owned)
        .collect();
    out.push(format!("dbname={dbname}"));
    out.join(" ")
}
