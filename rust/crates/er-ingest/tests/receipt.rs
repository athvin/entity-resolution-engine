//! Delivery-receipt classification (S4.1): drive a `raw_records` session table
//! through five batches and confirm `batch_receipt` labels every delivered key
//! as new / unchanged / changed / resurrected and every retired key as a
//! tombstone. `raw_records` is append-only, so the receipt is a post-landing
//! comparison against the latest prior row per key.

use dedupe_core::session::{Session, SessionOptions};
use er_config::load_config_str;
use er_ingest::landing::{
    BatchReceipt, batch_receipt, create_raw_records_sql, land_base_delivery, land_full_refresh,
};

const CRM_BASE: &str = include_str!("fixtures/base_10/base/crm.csv");

// C001's phone changes (a mapped column → new content hash); C999 is brand new.
const CRM_CHANGE: &str = "\
crm_id,first_name,last_name,email_address,phone,street_address,city,state,zip,dob,last_modified,persona_id
C001,Robert,Chen,robert.chen@example.com,(628) 555-9999,1420 Judah Street Apt 5,San Francisco,CA,94107,1985-03-14,2024-02-01 08:12:00,P1
C999,New,Person,new.person@example.com,(415) 555-1234,1 New St,San Francisco,CA,94100,1991-01-01,2024-02-01 09:00:00,P9
";

// A full-refresh carrying only C002, byte-identical to its base row.
const CRM_ONLY_C002: &str = "\
crm_id,first_name,last_name,email_address,phone,street_address,city,state,zip,dob,last_modified,persona_id
C002,Priya,Raghavan,priya.raghavan@example.com,(415) 555-0132,88 Harbor Street #4B,San Francisco,CA,94110,1990-07-22,2024-01-09 11:00:00,P2
";

// C001 re-delivered after it was tombstoned by the full refresh.
const CRM_RESURRECT: &str = "\
crm_id,first_name,last_name,email_address,phone,street_address,city,state,zip,dob,last_modified,persona_id
C001,Robert,Chen,robert.chen@example.com,(628) 555-0101,1420 Judah Street Apt 5,San Francisco,CA,94107,1985-03-14,2024-03-01 08:12:00,P1
";

#[test]
fn receipt_classifies_every_delivery_category() {
    let config = load_config_str(include_str!("fixtures/default.yaml")).unwrap();
    let spec = &config.sources["crm"];
    let session = Session::open_in_memory(SessionOptions::deterministic()).unwrap();
    session
        .execute_batch("ATTACH ':memory:' AS lake;")
        .and_then(|()| session.execute_batch(&create_raw_records_sql("lake.main.raw_records")))
        .unwrap();

    // b1 (base): every key is new.
    let landed = land_base_delivery(&session, "crm", spec, CRM_BASE, "b1", "2026-01-01").unwrap();
    let n_base = i64::try_from(landed).unwrap();
    assert_eq!(
        batch_receipt(&session, "crm", "b1").unwrap(),
        BatchReceipt {
            new: n_base,
            ..BatchReceipt::default()
        },
    );

    // b2: re-deliver the same bytes → all unchanged.
    land_base_delivery(&session, "crm", spec, CRM_BASE, "b2", "2026-01-02").unwrap();
    assert_eq!(
        batch_receipt(&session, "crm", "b2").unwrap(),
        BatchReceipt {
            unchanged: n_base,
            ..BatchReceipt::default()
        },
    );

    // b3: C001's content changes, C999 is new (the batch carries only these two).
    land_base_delivery(&session, "crm", spec, CRM_CHANGE, "b3", "2026-01-03").unwrap();
    assert_eq!(
        batch_receipt(&session, "crm", "b3").unwrap(),
        BatchReceipt {
            new: 1,
            changed: 1,
            ..BatchReceipt::default()
        },
    );

    // b4 (full refresh carrying only C002): C002 unchanged, every other live key
    // (the base keys besides C002, plus C999) is tombstoned.
    land_full_refresh(&session, "crm", spec, CRM_ONLY_C002, "b4", "2026-01-04").unwrap();
    assert_eq!(
        batch_receipt(&session, "crm", "b4").unwrap(),
        BatchReceipt {
            unchanged: 1,
            tombstone: n_base,
            ..BatchReceipt::default()
        },
    );

    // b5: re-deliver the tombstoned C001 → resurrected.
    land_base_delivery(&session, "crm", spec, CRM_RESURRECT, "b5", "2026-01-05").unwrap();
    assert_eq!(
        batch_receipt(&session, "crm", "b5").unwrap(),
        BatchReceipt {
            resurrected: 1,
            ..BatchReceipt::default()
        },
    );
}
