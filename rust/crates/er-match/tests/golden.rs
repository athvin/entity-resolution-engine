//! The golden-record gate: assemble base_10's golden records (survivorship) in
//! Rust and reproduce the committed `golden_lineage` (`lineage.csv`) and
//! `golden_records` (`golden.csv`).
//!
//! Resolve base_10 end-to-end, materialize `entity_membership`/`entities` from
//! the reconcile plan, then run the `er-golden` SQL generators. `lineage` is
//! compared by entity signature — the set of `(attribute, record_key, rule)` per
//! entity — so Rust's minted ids and the golden's `E*` labels drop out while the
//! WINNER and the DECIDING RULE are checked exactly. `golden_records` is compared
//! in SQL (ID dropped) so the `\N` null (E9's email) and JSON metadata compare
//! with DuckDB's own semantics.

mod common;

use std::collections::{BTreeMap, BTreeSet};

use dedupe_core::session::{Session, SessionOptions};
use er_golden::{golden_lineage_sql, golden_records_sql};

const GOLDEN_CSV: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/golden_base_10.csv"
);

/// The attribute columns compared in `golden_records` — every column except the
/// entity id (dropped for ID-insensitivity), in `golden.csv` order.
const GOLDEN_COLUMNS: [&str; 13] = [
    "given_name",
    "family_name",
    "email",
    "phone_e164",
    "addr_number",
    "addr_street",
    "addr_unit",
    "addr_city",
    "addr_region",
    "addr_postal",
    "birth_date",
    "metadata",
    "survivorship_version",
];

/// Group `(entity, attribute, record_key, rule)` rows into one signature set per
/// entity, keyed by nothing id-dependent.
type Signature = BTreeSet<(String, String, String)>;

fn signatures<I: IntoIterator<Item = (String, (String, String, String))>>(
    rows: I,
) -> BTreeSet<Signature> {
    let mut by_entity: BTreeMap<String, Signature> = BTreeMap::new();
    for (entity, triple) in rows {
        by_entity.entry(entity).or_default().insert(triple);
    }
    by_entity.into_values().collect()
}

#[test]
fn base_10_golden_lineage_and_records_match_committed() {
    let session = Session::open_in_memory(SessionOptions::deterministic()).unwrap();
    let (config, plan) = common::resolve_base_10(&session);
    common::materialize_entities(&session, &plan);

    let lineage_sql = golden_lineage_sql(
        &config,
        "int_std_records",
        "entity_membership",
        "entities",
        "2026-01-01 00:00:00",
    )
    .unwrap();
    session
        .execute_batch(&format!("create table golden_lineage as\n{lineage_sql}"))
        .unwrap();
    let records_sql = golden_records_sql(
        &config,
        "golden_lineage",
        "int_std_records",
        "entity_membership",
        "entities",
        "2026-01-01 00:00:00",
    );
    session
        .execute_batch(&format!("create table golden_records as\n{records_sql}"))
        .unwrap();

    // --- golden_lineage: winner + deciding rule per (entity, attribute) ---
    let got_lineage = signatures(
        session
            .query_strings("select entity_id, attribute, record_key, rule from golden_lineage")
            .unwrap()
            .into_iter()
            .map(|r| (r[0].clone(), (r[1].clone(), r[2].clone(), r[3].clone()))),
    );
    let want_lineage = signatures(
        include_str!("fixtures/lineage_base_10.csv")
            .lines()
            .skip(1)
            .filter(|l| !l.is_empty())
            .map(|line| {
                // entity_label, attribute, record_key, source_system, source_record_id, rule, ver
                let f: Vec<&str> = line.split(',').collect();
                (
                    f[0].to_string(),
                    (f[1].to_string(), f[2].to_string(), f[5].to_string()),
                )
            }),
    );
    assert_eq!(
        got_lineage.len(),
        10,
        "golden_lineage should cover 10 entities"
    );
    assert_eq!(
        got_lineage, want_lineage,
        "golden_lineage (winner/rule per attribute) differs from the committed golden"
    );

    // --- golden_records: assembled values, compared in SQL (id dropped) ---
    let golden_path = GOLDEN_CSV.replace('\'', "''");
    let columns_decl = GOLDEN_COLUMNS
        .iter()
        .map(|c| format!("'{c}': 'VARCHAR'"))
        .collect::<Vec<_>>()
        .join(", ");
    session
        .execute_batch(&format!(
            "create table golden_expected as select * from read_csv('{golden_path}', \
             header = true, nullstr = '\\N', \
             columns = {{'entity_label': 'VARCHAR', {columns_decl}}})"
        ))
        .unwrap();

    let projection = GOLDEN_COLUMNS
        .iter()
        .map(|c| format!("cast({c} as varchar) as {c}"))
        .collect::<Vec<_>>()
        .join(", ");
    let diff = session
        .count(&format!(
            "(
               (select {projection} from golden_records
                  except
                select {cols} from golden_expected)
               union all
               (select {cols} from golden_expected
                  except
                select {projection} from golden_records)
             )",
            cols = GOLDEN_COLUMNS.join(", "),
        ))
        .unwrap();
    assert_eq!(
        diff, 0,
        "{diff} golden_records rows differ from golden.csv (attribute columns, id-insensitive)"
    );
}
