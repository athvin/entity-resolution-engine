//! Shared setup for the matching gates: land base_10's base delivery and build
//! `int_std_records` in a dedupe-core session — the standardized corpus the
//! matching stage scores. Standardization itself is gated byte-exact vs Python
//! in `er-standardize` (std_hashes.rs over this same base_10 delivery), so these
//! matching gates take that corpus as given and prove the predict/score layer.

use dedupe_core::session::Session;
use er_config::load_config_str;
use er_ingest::landing::{create_raw_records_sql, land_base_delivery};
use er_standardize::build_int_std_records;

/// Build `int_std_records` for base_10's base delivery into `session`, returning
/// the config (the committed model is scored against this config's standardized
/// output). Leaves a table named `int_std_records` in the session.
pub fn land_and_standardize_base_10(session: &Session) -> er_config::Config {
    let config = load_config_str(include_str!("../fixtures/test.yaml")).unwrap();
    session
        .execute_batch(&create_raw_records_sql("raw_records"))
        .unwrap();

    // Seed the nickname lexicon the `name_variants` relation reads.
    let values: Vec<String> = include_str!("../fixtures/nickname_variants.csv")
        .lines()
        .skip(1)
        .filter(|l| !l.is_empty())
        .map(|line| {
            let (a, b) = line.split_once(',').unwrap();
            format!(
                "('{}', '{}', true)",
                a.replace('\'', "''"),
                b.replace('\'', "''")
            )
        })
        .collect();
    session
        .execute_batch(&format!(
            "create table nickname_variants as select * from (values {}) \
             as t(variant_a, variant_b, active)",
            values.join(", ")
        ))
        .unwrap();

    for (source, csv_text) in [
        ("crm", include_str!("../fixtures/base_10/base/crm.csv")),
        (
            "billing",
            include_str!("../fixtures/base_10/base/billing.csv"),
        ),
        (
            "webforms",
            include_str!("../fixtures/base_10/base/webforms.csv"),
        ),
    ] {
        let spec = &config.sources[source];
        land_base_delivery(
            session,
            "raw_records",
            source,
            spec,
            csv_text,
            "01BATCH0000000000000000BASE",
            "2026-01-01 00:00:00",
        )
        .unwrap();
    }

    build_int_std_records(
        session,
        &config,
        "raw_records",
        "nickname_variants",
        "int_std_records",
    )
    .unwrap();

    config
}
