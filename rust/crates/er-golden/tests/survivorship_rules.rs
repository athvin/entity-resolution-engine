//! PR 1.6 Bucket A: the new single-winner survivorship rule tokens elect the
//! expected record. Runs the generated `survivorship_decision_sql` over an
//! in-memory DuckDB session with a tiny candidate table. These rules reference
//! only `value`/`entity_id`/`record_key`, so no sources/validity columns are
//! needed. (End-to-end over the real lake for the existing rules is gated by
//! er-match's base_10 golden test, which these additive tokens leave untouched.)

use dedupe_core::session::{Session, SessionOptions};
use er_golden::survivorship_decision_sql;

/// Elect the rank-1 winner for `chain` over `rows` (entity_id, value, record_key).
/// Returns (value, record_key, rule).
fn elect(rows: &[(&str, &str, &str)], chain: &[&str]) -> (String, String, String) {
    let session = Session::open_in_memory(SessionOptions::deterministic()).unwrap();
    session
        .execute_batch(
            "CREATE OR REPLACE TABLE cand(entity_id VARCHAR, value VARCHAR, record_key VARCHAR)",
        )
        .unwrap();
    let values = rows
        .iter()
        .map(|(e, v, k)| format!("('{e}', '{v}', '{k}')"))
        .collect::<Vec<_>>()
        .join(", ");
    session
        .execute_batch(&format!("INSERT INTO cand VALUES {values}"))
        .unwrap();
    let chain: Vec<String> = chain.iter().map(|s| (*s).to_string()).collect();
    let sql = survivorship_decision_sql("value", &chain, "cand").unwrap();
    let out = session.query_strings(&sql).unwrap();
    assert_eq!(out.len(), 1, "one winner row per entity");
    let r = &out[0];
    (r[1].clone(), r[2].clone(), r[3].clone())
}

#[test]
fn longest_elects_the_longest_value() {
    let (value, key, rule) = elect(
        &[
            ("e1", "ab", "r1"),
            ("e1", "abcd", "r2"),
            ("e1", "abc", "r3"),
        ],
        &["longest", "record_key ASC"],
    );
    assert_eq!(
        (value.as_str(), key.as_str(), rule.as_str()),
        ("abcd", "r2", "longest")
    );
}

#[test]
fn shortest_elects_the_shortest_value() {
    let (_, key, rule) = elect(
        &[
            ("e1", "ab", "r1"),
            ("e1", "abcd", "r2"),
            ("e1", "abc", "r3"),
        ],
        &["shortest", "record_key ASC"],
    );
    assert_eq!((key.as_str(), rule.as_str()), ("r1", "shortest"));
}

#[test]
fn numeric_max_and_min_cast_before_comparing() {
    // String order would put '5' > '20'; numeric must pick 20 / 3.
    let rows = [("e1", "5", "r1"), ("e1", "20", "r2"), ("e1", "3", "r3")];
    let (vmax, kmax, rmax) = elect(&rows, &["numeric_max", "record_key ASC"]);
    assert_eq!(
        (vmax.as_str(), kmax.as_str(), rmax.as_str()),
        ("20", "r2", "numeric_max")
    );
    let (vmin, kmin, rmin) = elect(&rows, &["numeric_min", "record_key ASC"]);
    assert_eq!(
        (vmin.as_str(), kmin.as_str(), rmin.as_str()),
        ("3", "r3", "numeric_min")
    );
}

#[test]
fn most_common_elects_the_modal_value() {
    let (value, _, _) = elect(
        &[("e1", "x", "r1"), ("e1", "x", "r2"), ("e1", "y", "r3")],
        &["most_common", "record_key ASC"],
    );
    assert_eq!(value, "x", "the value held by the most member records wins");
}

#[test]
fn prioritized_ranks_by_the_configured_list() {
    // Ranked b > a > c; unlisted sorts last.
    let (value, key, rule) = elect(
        &[
            ("e1", "a", "r1"),
            ("e1", "b", "r2"),
            ("e1", "c", "r3"),
            ("e1", "z", "r4"),
        ],
        &["prioritized:b|a|c", "record_key ASC"],
    );
    assert_eq!(
        (value.as_str(), key.as_str(), rule.as_str()),
        ("b", "r2", "prioritized:b|a|c")
    );
}

#[test]
fn prioritized_falls_through_when_no_value_is_listed() {
    // None listed -> all tie on the rule -> terminal record_key ASC decides.
    let (_, key, rule) = elect(
        &[("e1", "p", "r2"), ("e1", "q", "r1")],
        &["prioritized:x|y", "record_key ASC"],
    );
    assert_eq!(
        (key.as_str(), rule.as_str()),
        ("r1", "tiebreak_deterministic")
    );
}

// --- PR 1.6b (Bucket B): value-synthesizing rules. The decision's `value` column
// IS the synthesized value (combined_list / sum); `record_key` is the
// deterministic representative (`min`) so lineage still names a member, and the
// `rule` is the synthesizing rule. (Record-assembly over the real lake reuses the
// same aggregate via `synthesized_value_expr`.) ---

#[test]
fn combined_list_unions_distinct_sorted_values() {
    // Duplicates collapse, blanks drop, output is sorted and '; '-joined.
    let (value, key, rule) = elect(
        &[
            ("e1", "b", "r2"),
            ("e1", "a", "r1"),
            ("e1", "b", "r3"),
            ("e1", "", "r4"),
        ],
        &["combined_list"],
    );
    assert_eq!(
        (value.as_str(), key.as_str(), rule.as_str()),
        ("a; b", "r1", "combined_list")
    );
}

#[test]
fn sum_totals_numeric_values_dropping_non_numeric() {
    // 10 + 5 = 15; "x" casts to NULL and drops out; representative is min key.
    let (value, key, rule) = elect(
        &[("e1", "10", "r1"), ("e1", "5", "r2"), ("e1", "x", "r3")],
        &["sum"],
    );
    assert_eq!(
        (value.as_str(), key.as_str(), rule.as_str()),
        ("15.0", "r1", "sum")
    );
}

#[test]
fn a_synthesizing_rule_cannot_be_mixed_with_ranking_rules() {
    let chain = ["combined_list".to_string(), "longest".to_string()];
    let err = survivorship_decision_sql("value", &chain, "cand").unwrap_err();
    assert!(
        err.to_string().contains("only"),
        "expected a mixing error, got: {err}"
    );
}

#[test]
fn a_synthesizing_rule_is_rejected_on_the_composite_address() {
    let chain = ["combined_list".to_string()];
    let err = survivorship_decision_sql("address", &chain, "cand").unwrap_err();
    assert!(
        err.to_string().contains("address"),
        "expected an address error, got: {err}"
    );
}

// ---------------------------------------------------------------------------
// 1.6c: field sets that survive together.
// ---------------------------------------------------------------------------

use er_config::load_config_str;
use er_golden::lineage::{resolved_groups, resolved_scalars};

const BASE: &str = include_str!("../../er-config/tests/fixtures/test.yaml");

/// `BASE` with groups declared on the parsed config.
///
/// Built by mutating the struct rather than by splicing YAML: a grouped field
/// must not keep its own chain (the publish check refuses that), and removing a
/// key from a `BTreeMap` is exact where editing text is not.
fn with_groups(groups: &[(&str, &[&str], Option<&str>)]) -> er_config::Config {
    let mut config = load_config_str(BASE).expect("base config parses");
    for (token, fields, controlling) in groups {
        for field in *fields {
            config.survivorship.remove(*field);
        }
        config
            .survivorship
            .entry((*token).to_string())
            .or_insert_with(|| {
                vec![
                    "source_priority".to_string(),
                    "recency".to_string(),
                    "record_key ASC".to_string(),
                ]
            });
        config.survivorship_groups.insert(
            (*token).to_string(),
            er_config::SurvivorshipGroup {
                fields: fields.iter().map(|f| (*f).to_string()).collect(),
                controlling: controlling.map(ToString::to_string),
            },
        );
    }
    config
}

#[test]
fn an_empty_block_resolves_to_exactly_the_builtin_address_group() {
    // The determinism obligation of 1.6c, stated as a test rather than trusted:
    // every golden fixture pins the generated SQL, and the SQL is built from
    // these two lists.
    let config = load_config_str(BASE).unwrap();
    let groups = resolved_groups(&config, false);
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].token, "address");
    assert_eq!(
        groups[0].fields,
        vec![
            "addr_number",
            "addr_street",
            "addr_unit",
            "addr_city",
            "addr_region",
            "addr_postal"
        ]
    );
    assert!(
        groups[0].controlling.is_none(),
        "the built-in address group ranks on the whole composite, as it always has"
    );
    assert_eq!(
        resolved_scalars(&config, false),
        vec![
            "given_name",
            "family_name",
            "email",
            "phone_e164",
            "birth_date"
        ],
        "with no groups declared the scalar list is the whole vocabulary, in order"
    );
}

#[test]
fn a_declared_group_claims_its_fields_from_the_scalar_list() {
    // The motivating case: `given_name` and `family_name` are elected
    // independently today, so a golden record can take one from each source and
    // describe a person who never existed.
    let config = with_groups(&[(
        "full_name",
        &["given_name", "family_name"],
        Some("family_name"),
    )]);
    let scalars = resolved_scalars(&config, false);
    assert!(
        !scalars.contains(&"given_name") && !scalars.contains(&"family_name"),
        "a grouped field must stop being elected on its own: {scalars:?}"
    );
    assert_eq!(scalars, vec!["email", "phone_e164", "birth_date"]);

    let groups = resolved_groups(&config, false);
    let tokens: Vec<&str> = groups.iter().map(|g| g.token).collect();
    assert_eq!(
        tokens,
        vec!["address", "full_name"],
        "the built-in address group survives alongside a declared one, in token order"
    );
    let full_name = groups.iter().find(|g| g.token == "full_name").unwrap();
    assert_eq!(full_name.controlling, Some("family_name"));
}

#[test]
fn naming_address_overrides_the_builtin_rather_than_duplicating_it() {
    // Otherwise two relations called `attr_address` would be generated — the
    // config would be accepted and the SQL would not compile.
    let config = with_groups(&[(
        "address",
        &["addr_city", "addr_region", "addr_postal"],
        Some("addr_postal"),
    )]);
    let groups = resolved_groups(&config, false);
    assert_eq!(
        groups.len(),
        1,
        "exactly one address group: {:?}",
        groups.iter().map(|g| g.token).collect::<Vec<_>>()
    );
    assert_eq!(
        groups[0].fields,
        vec!["addr_city", "addr_region", "addr_postal"]
    );
    assert_eq!(groups[0].controlling, Some("addr_postal"));
}

#[test]
fn the_account_vocabulary_comes_from_the_caller_not_the_config() {
    // The Account builders generate for Account by construction. Reading the
    // vocabulary off `config.entity_type` instead let a defaulted config emit
    // person columns into Account SQL — caught by er-sql-check's account mart
    // test as `No field named r.given_name`, a planning failure a long way
    // from its cause.
    let config = load_config_str(BASE).unwrap();
    assert_eq!(
        resolved_scalars(&config, true),
        vec!["company_name", "domain", "phone_e164"]
    );
    assert_eq!(
        resolved_groups(&config, true)[0].fields,
        vec!["addr_street", "addr_city", "addr_region", "addr_postal"],
        "Account has no addr_number/addr_unit"
    );
}

#[test]
fn a_group_takes_every_field_from_one_record() {
    // The property, executed rather than asserted about SQL text. Two records
    // where the "best" given_name and the "best" family_name live in DIFFERENT
    // records: elected independently they combine into a name nobody has.
    let config = with_groups(&[(
        "full_name",
        &["given_name", "family_name"],
        Some("family_name"),
    )]);
    let session = Session::open_in_memory(SessionOptions::deterministic()).unwrap();
    session
        .execute_batch(
            "CREATE OR REPLACE TABLE cand(
                 entity_id VARCHAR, record_key VARCHAR,
                 given_name VARCHAR, family_name VARCHAR,
                 value VARCHAR)",
        )
        .unwrap();
    // `value` is the controlling field's value, which is what the chain ranks.
    session
        .execute_batch(
            "INSERT INTO cand VALUES
               ('e1', 'r1', 'Bob',    'Chen',      'Chen'),
               ('e1', 'r2', 'Robert', 'Chenoweth', 'Chenoweth')",
        )
        .unwrap();
    let chain = vec!["longest".to_string(), "record_key ASC".to_string()];
    let sql = survivorship_decision_sql("full_name", &chain, "cand").unwrap();
    let out = session.query_strings(&sql).unwrap();
    assert_eq!(out.len(), 1);
    let winner = &out[0][2];
    assert_eq!(
        winner, "r2",
        "the chain ranks on the controlling field, so the longer family name wins"
    );

    // ...and the group projection reads BOTH fields from that one winner, which
    // is the whole point: `Robert Chenoweth`, never `Robert Chen`.
    let pairs = session
        .query_strings(&format!(
            "with d as ({sql}) select c.given_name, c.family_name \
             from d join cand c on c.record_key = d.record_key"
        ))
        .unwrap();
    assert_eq!(
        pairs[0],
        vec!["Robert".to_string(), "Chenoweth".to_string()]
    );

    // The group's token is what lineage records, so the projection filters on it.
    let projection = er_golden::golden_records_sql(
        &config,
        "lake.main.golden_lineage",
        "lake.main.int_std_records",
        "lake.main.entity_membership",
        "lake.main.entities",
        "2026-01-01 00:00:00",
    );
    assert!(
        projection.contains("filter (where l.attribute = 'full_name') as given_name"),
        "given_name must be read from the full_name decision, not its own"
    );
    assert!(
        projection.contains("filter (where l.attribute = 'full_name') as family_name"),
        "family_name must be read from the same decision as given_name"
    );
}
