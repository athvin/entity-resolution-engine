//! The DSN rewriting behind one-database-per-test. Needs no database.
//!
//! These assertions live here rather than in a `#[cfg(test)]` module inside
//! `common/mod.rs`, because that file is `mod common;`-ed into every `pg_*.rs`
//! binary and a test inside it would run — and be reported — once per binary.

mod common;

#[test]
fn repoints_the_database_and_keeps_every_other_parameter() {
    assert_eq!(
        common::with_dbname(
            "host=localhost port=5466 user=er password=er dbname=er",
            "er_t_tags"
        ),
        "host=localhost port=5466 user=er password=er dbname=er_t_tags"
    );
}

#[test]
fn names_a_database_when_the_dsn_named_none() {
    assert_eq!(
        common::with_dbname("host=localhost user=er", "er_t_x"),
        "host=localhost user=er dbname=er_t_x"
    );
}

#[test]
#[should_panic(expected = "keyword/value form")]
fn refuses_a_url_dsn_rather_than_connecting_somewhere_unintended() {
    // Silently leaving `dbname` alone would point every test at the shared
    // database again — the exact failure this module exists to remove — so the
    // unsupported form has to be loud.
    let _ = common::with_dbname("postgresql://er:er@localhost:5466/er", "er_t_x");
}
