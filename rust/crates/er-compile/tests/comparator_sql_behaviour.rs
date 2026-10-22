//! The type-aware comparators, evaluated by DuckDB rather than eyeballed.
//!
//! A comparator is a string of SQL. Asserting that the compiler *emits* the
//! expected string proves only that the compiler is self-consistent — it cannot
//! catch SQL that is valid but means the wrong thing, which is the failure mode
//! that matters here. A date comparator that silently compares as text, or a
//! numeric one that divides by zero on equal zeros, would pass a string
//! assertion and be wrong in production.
//!
//! So each case below builds a two-row table, runs the generated predicate over
//! it, and asserts which pairs it admits. Gated behind `DEDUPE_E2E_EXTENSIONS`
//! like the other tests that need a real engine session.

use dedupe_core::Session;
use dedupe_core::session::SessionOptions;

fn enabled() -> bool {
    std::env::var("DEDUPE_E2E_EXTENSIONS").as_deref() == Ok("1")
        || std::env::var("DEDUPE_E2E_DUCKLAKE").as_deref() == Ok("1")
}

/// Evaluate a compiled level's SQL over one `(left, right)` pair.
///
/// The predicate is written in terms of `"<col>_l"` / `"<col>_r"`, which is how
/// the engine presents a candidate pair, so the harness builds exactly that
/// shape — the same column naming the real comparison runs against.
fn admits(
    session: &Session,
    column: &str,
    sql_type: &str,
    left: &str,
    right: &str,
    predicate: &str,
) -> bool {
    let table = "pair_probe";
    session
        .execute_batch(&format!("drop table if exists {table}"))
        .unwrap();
    session
        .execute_batch(&format!(
            "create table {table} as select \
               cast({left} as {sql_type}) as \"{column}_l\", \
               cast({right} as {sql_type}) as \"{column}_r\""
        ))
        .unwrap();
    let rows = session
        .query_strings(&format!(
            "select case when ({predicate}) then 'yes' else 'no' end from {table}"
        ))
        .unwrap_or_else(|e| panic!("predicate did not run: {e}\n  sql: {predicate}"));
    rows[0][0] == "yes"
}

fn level_sql(column: &str, token: &str) -> String {
    // Compile a one-level comparison and read back the level's SQL, so the test
    // exercises the same path the model emitter uses.
    let spec = er_config::ComparisonSpec {
        levels: vec![Some(token.to_string())],
        tf: false,
        weight: None,
        null_policy: er_config::NullPolicy::Never,
    };
    let mut config =
        er_config::load_config_str(include_str!("../../er-config/tests/fixtures/test.yaml"))
            .unwrap();
    config.comparisons.clear();
    config.comparisons.insert(column.to_string(), spec);
    let compiled = er_compile::compile(&config).expect("the probe config compiles");
    let settings: serde_json::Value = serde_json::from_str(&compiled.model_json).unwrap();
    let levels = settings["comparisons"][0]["comparison_levels"]
        .as_array()
        .unwrap();
    // [0] is the null level, [1] is the declared one, last is ELSE.
    levels[1]["sql_condition"].as_str().unwrap().to_string()
}

fn session() -> Session {
    Session::open_in_memory(SessionOptions::deterministic()).unwrap()
}

#[test]
fn a_date_tolerance_compares_dates_not_strings() {
    if !enabled() {
        eprintln!("skipping comparator SQL behaviour (set DEDUPE_E2E_EXTENSIONS=1)");
        return;
    }
    let session = session();
    let sql = level_sql("birth_date", "date_within_days:2");

    let probe = |l: &str, r: &str| admits(&session, "birth_date", "DATE", l, r, &sql);
    assert!(
        probe("'1985-03-14'", "'1985-03-14'"),
        "same day is within 2"
    );
    assert!(probe("'1985-03-14'", "'1985-03-16'"), "two days later");
    assert!(probe("'1985-03-14'", "'1985-03-12'"), "two days earlier");
    assert!(
        !probe("'1985-03-14'", "'1985-03-20'"),
        "six days is outside"
    );
    // The case string comparison gets wrong: a month boundary. These are one
    // day apart as dates and wildly different as text.
    assert!(
        probe("'1985-03-31'", "'1985-04-01'"),
        "across a month boundary one day apart must be within 2"
    );
    // And the inverse: textually adjacent, actually a year apart.
    assert!(
        !probe("'1985-03-14'", "'1986-03-14'"),
        "same day next year is not within 2 days"
    );
}

#[test]
fn a_transposed_date_is_caught_though_nothing_else_would() {
    if !enabled() {
        return;
    }
    let session = session();
    let sql = level_sql("birth_date", "date_transposed");
    let probe = |l: &str, r: &str| admits(&session, "birth_date", "DATE", l, r, &sql);

    // A day/month transposition is only *representable* when both numbers are
    // 12 or less — `1985-14-03` is not a date and would never have been stored.
    // That is also exactly when the ambiguity arises in the first place: a
    // record entered from "04/03/1985" is 4 March or 3 April depending on whose
    // convention the form used, and both readings are valid dates.
    assert!(
        probe("'1985-03-04'", "'1985-04-03'"),
        "4 March and 3 April are the same entry under the two conventions"
    );
    assert!(probe("'1985-04-03'", "'1985-03-04'"), "and symmetrically");

    // Neither of the comparators that already existed can see this. String
    // similarity is high but the dates are a month apart; a day-tolerance window
    // would have to be ~30 days wide to catch it, which would also catch
    // genuinely different people.
    let within_2 = level_sql("birth_date", "date_within_days:2");
    assert!(
        !admits(
            &session,
            "birth_date",
            "DATE",
            "'1985-03-04'",
            "'1985-04-03'",
            &within_2
        ),
        "a 2-day window must NOT catch a transposition — that is why this level exists"
    );

    // The same year is required: a transposition is a reading error, not a
    // different date.
    assert!(!probe("'1985-03-04'", "'1986-04-03'"), "different years");
    // And genuinely different days are not transpositions.
    assert!(!probe("'1985-03-14'", "'1985-07-22'"));
    // A date whose day equals its month is its own transposition, which is
    // harmless: the exact level already matches it and this one agrees.
    assert!(probe("'1985-05-05'", "'1985-05-05'"));
}

#[test]
fn numeric_tolerances_work_absolutely_and_relatively() {
    if !enabled() {
        return;
    }
    let session = session();

    let abs_sql = level_sql("addr_postal", "numeric_within:2");
    let abs_probe = |l: &str, r: &str| admits(&session, "addr_postal", "VARCHAR", l, r, &abs_sql);
    assert!(abs_probe("'94107'", "'94108'"), "one apart is within 2");
    assert!(!abs_probe("'94107'", "'94999'"), "892 apart is not");
    // Non-numeric text must not blow up or match — `try_cast` returns NULL and
    // the guard rejects it.
    assert!(
        !abs_probe("'SW1A 1AA'", "'SW1A 1AB'"),
        "non-numeric values must be rejected, not error"
    );

    let pct_sql = level_sql("addr_postal", "numeric_within_pct:10");
    let pct_probe = |l: &str, r: &str| admits(&session, "addr_postal", "VARCHAR", l, r, &pct_sql);
    assert!(pct_probe("'100'", "'105'"), "5% apart is within 10%");
    assert!(!pct_probe("'100'", "'130'"), "30% apart is not");
    // The scale-independence that makes a relative tolerance worth having.
    assert!(pct_probe("'50000'", "'52000'"), "4% of a large value");
    // Two zeros: the denominator guard must stop a divide-by-zero rather than
    // the query failing. They are equal, so the exact level would catch them —
    // what matters is that this predicate RUNS.
    assert!(
        !pct_probe("'0'", "'0'"),
        "equal zeros are excluded by the positive-denominator guard, not an error"
    );
}

#[test]
fn address_components_agree_where_string_similarity_would_hesitate() {
    if !enabled() {
        return;
    }
    let session = session();
    let sql = level_sql("addr_postal", "address_components");

    // This level reads the parsed components, so the probe table needs them all
    // rather than one column pair.
    session
        .execute_batch("drop table if exists pair_probe")
        .unwrap();
    let build = |ln: &str, ls: &str, lp: &str, rn: &str, rs: &str, rp: &str| {
        session
            .execute_batch("drop table if exists pair_probe")
            .unwrap();
        session
            .execute_batch(&format!(
                "create table pair_probe as select \
                   {ln} as addr_number_l, {ls} as addr_street_l, {lp} as addr_postal_l, \
                   {rn} as addr_number_r, {rs} as addr_street_r, {rp} as addr_postal_r"
            ))
            .unwrap();
    };
    // The level's SQL is written with `l.`/`r.` aliases, so probe it through a
    // join-shaped query the way the engine evaluates it.
    let probe = |ln: &str, ls: &str, lp: &str, rn: &str, rs: &str, rp: &str| -> bool {
        build(ln, ls, lp, rn, rs, rp);
        let rows = session
            .query_strings(&format!(
                "select case when ({sql}) then 'yes' else 'no' end from ( \
                   select addr_number_l as addr_number, addr_street_l as addr_street, \
                          addr_postal_l as addr_postal from pair_probe) l, ( \
                   select addr_number_r as addr_number, addr_street_r as addr_street, \
                          addr_postal_r as addr_postal from pair_probe) r"
            ))
            .unwrap_or_else(|e| panic!("predicate did not run: {e}\n  sql: {sql}"));
        rows[0][0] == "yes"
    };

    assert!(
        probe(
            "'1420'", "'judah'", "'94107'", "'1420'", "'judah'", "'94107'"
        ),
        "identical components agree"
    );
    assert!(
        !probe(
            "'1420'", "'judah'", "'94107'", "'1422'", "'judah'", "'94107'"
        ),
        "a different street number is a different address"
    );
    assert!(
        !probe("NULL", "'judah'", "'94107'", "NULL", "'judah'", "'94107'"),
        "a missing number must not count as agreement"
    );
}

#[test]
fn a_bad_comparator_parameter_is_refused_at_compile_time() {
    // No engine needed: these never reach SQL.
    for token in [
        "date_within_days:soon",
        "numeric_within:-1",
        "numeric_within_pct:150",
        "numeric_within_pct:abc",
    ] {
        let spec = er_config::ComparisonSpec {
            levels: vec![Some(token.to_string())],
            tf: false,
            weight: None,
            null_policy: er_config::NullPolicy::Never,
        };
        let mut config =
            er_config::load_config_str(include_str!("../../er-config/tests/fixtures/test.yaml"))
                .unwrap();
        config.comparisons.clear();
        config.comparisons.insert("birth_date".to_string(), spec);
        assert!(
            er_compile::compile(&config).is_err(),
            "{token:?} should be refused before it can become SQL"
        );
    }
}

/// Evaluate a constraint conjunct over one `(left, right)` pair.
///
/// Constraints are written in terms of `l.<col>` / `r.<col>` — the two sides of
/// a blocking join, not the `_l`/`_r` pair columns a comparison level sees — so
/// this builds that shape instead of reusing [`admits`].
///
/// The `case when … then 'yes' else 'no'` wrapper is the point: it reproduces
/// what a WHERE clause does to a NULL conjunct, which is to drop the row. A
/// three-valued conjunct that evaluates to NULL is NOT a pair that is "maybe
/// eligible" — it is a pair the engine never generates.
fn pair_eligible(session: &Session, column: &str, left: &str, right: &str, conjunct: &str) -> bool {
    session
        .execute_batch("drop table if exists l; drop table if exists r")
        .unwrap();
    for (alias, value) in [("l", left), ("r", right)] {
        session
            .execute_batch(&format!(
                "create table {alias} as select cast({value} as VARCHAR) as \"{column}\""
            ))
            .unwrap();
    }
    let rows = session
        .query_strings(&format!(
            "select case when ({conjunct}) then 'yes' else 'no' end from l, r"
        ))
        .unwrap_or_else(|e| panic!("conjunct did not run: {e}\n  sql: {conjunct}"));
    rows[0][0] == "yes"
}

#[test]
fn a_relationship_exclusion_is_symmetric_in_l_and_r() {
    // The bug this test exists for: `exclude_when_equal` guarded only the LEFT
    // side against NULL. With a right-hand NULL, `l.c = r.c` is NULL, so the
    // conjunct was `NOT (NULL AND TRUE)` = NULL, and a NULL conjunct in a WHERE
    // clause DROPS the pair. A record with a household could therefore never be
    // compared against one without — the exact opposite of the rule's stated
    // intent that "a NULL key excludes nothing".
    //
    // Why no unit test caught it: the existing truth tables mirror the SQL in
    // Rust, and Rust's `&&`/`!` are two-valued. The fault lived entirely in the
    // gap between two-valued Rust and three-valued SQL, so only running the
    // real string in a real engine can see it. That is also why this case sits
    // in this file rather than beside the other constraint unit tests.
    if !enabled() {
        eprintln!("skipping constraint conjunct SQL behaviour (set DEDUPE_E2E_EXTENSIONS=1)");
        return;
    }
    let session = session();

    let exclude = er_config::MatchConstraints {
        exclude_when_equal: vec!["household_id".to_string()],
        ..er_config::MatchConstraints::default()
    };
    let conjuncts = er_compile::constraint_conjuncts(&exclude);
    assert_eq!(conjuncts.len(), 1);
    let sql = &conjuncts[0];
    let probe = |l: &str, r: &str| pair_eligible(&session, "household_id", l, r, sql);

    assert!(
        !probe("'H'", "'H'"),
        "same household: related, not duplicate — excluded"
    );
    assert!(probe("'H'", "'K'"), "different households: eligible");
    assert!(probe("NULL", "NULL"), "neither has a household: eligible");
    // The two halves of the fix. Before it, the second of these was false.
    assert!(probe("NULL", "'H'"), "left missing: eligible");
    assert!(
        probe("'H'", "NULL"),
        "right missing: eligible — this is the asymmetry the rule had"
    );

    // And the same probe over `veto`, which was already correct, so the two
    // rules can be told apart rather than both being asserted to one shape.
    let veto = er_config::MatchConstraints {
        veto: vec!["ssn".to_string()],
        ..er_config::MatchConstraints::default()
    };
    let veto_sql = &er_compile::constraint_conjuncts(&veto)[0];
    let vetoed = |l: &str, r: &str| pair_eligible(&session, "ssn", l, r, veto_sql);
    assert!(vetoed("'1'", "'1'"), "agreeing ssn: eligible");
    assert!(!vetoed("'1'", "'2'"), "conflicting ssn: vetoed");
    assert!(vetoed("'1'", "NULL"), "missing on one side: tolerated");
    assert!(
        vetoed("NULL", "'1'"),
        "missing on the other side: tolerated"
    );
    assert!(vetoed("NULL", "NULL"), "missing on both: tolerated");

    // required_match, for the third shape: presence is required on BOTH sides,
    // which is what distinguishes it from veto.
    let required = er_config::MatchConstraints {
        required_match: vec!["tax_id".to_string()],
        ..er_config::MatchConstraints::default()
    };
    let required_sql = &er_compile::constraint_conjuncts(&required)[0];
    let needed = |l: &str, r: &str| pair_eligible(&session, "tax_id", l, r, required_sql);
    assert!(needed("'1'", "'1'"), "agreeing tax id: candidate");
    assert!(!needed("'1'", "'2'"), "conflicting tax id: not a candidate");
    assert!(
        !needed("'1'", "NULL"),
        "missing on one side: not a candidate"
    );
    assert!(
        !needed("NULL", "NULL"),
        "missing on both sides must NOT read as agreement"
    );
}
