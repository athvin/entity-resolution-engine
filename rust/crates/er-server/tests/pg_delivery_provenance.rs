//! #1296: delivery provenance — the durable link from a run to the Salesforce
//! connection and object its records came from. Idempotent on retry, refuses a
//! conflicting identity, absent for runs nothing recorded (uploads, pushes),
//! and exposes a source reused against a second connection as ambiguous.
//!
//! ONE test with sequential phases (the file-level rule `pg_serial.rs` checks).

use er_server::pg;

use crate::common;

#[tokio::test]
async fn provenance_is_idempotent_refuses_conflicts_and_flags_reused_sources() {
    let Some(db) = common::fresh_db("delivery_provenance").await else {
        return;
    };
    let mut client = pg::connect(db.dsn()).await.unwrap();
    pg::ensure_schema(&mut client).await.unwrap();
    pg::ensure_org(&client, "prov", "/tmp/prov.yaml")
        .await
        .unwrap();
    let org = pg::get_org(&client, "prov").await.unwrap().unwrap();
    let ds = pg::default_dataset_id(&client, &org.id)
        .await
        .unwrap()
        .unwrap();
    let mut connections = Vec::new();
    for name in ["prod", "sandbox"] {
        connections.push(
            pg::create_salesforce_connection(
                &client,
                &org.id,
                name,
                "https://example.my.salesforce.com",
                "client_credentials",
                "id",
                "secret",
                "v62.0",
            )
            .await
            .unwrap(),
        );
    }
    let row = |run: &'static str, delivery: &'static str, conn: usize| pg::DeliveryProvenance {
        org_id: &org.id,
        dataset_id: &ds,
        run_id: run,
        delivery_id: delivery,
        source: "crm",
        connection_id: &connections[conn],
        object: "Contact",
        delivery_sha256: "d1",
        source_sha256: "s1",
    };

    // Recorded once; a retry with the same identity converges.
    pg::record_provenance(&client, &row("run-1", "del-1", 0))
        .await
        .unwrap();
    pg::record_provenance(&client, &row("run-1", "del-1", 0))
        .await
        .unwrap();
    let got = pg::provenance_for_runs(&client, &org.id, &ds, &["run-1", "run-upload"])
        .await
        .unwrap();
    assert_eq!(got.len(), 1, "an upload's run has no provenance");
    assert_eq!(got["run-1"].salesforce_connection_id, connections[0]);

    // A different identity for the same run, or the same delivery bound to
    // another run, is refused and changes nothing.
    for conflicting in [row("run-1", "del-1", 1), row("run-2", "del-1", 0)] {
        let err = pg::record_provenance(&client, &conflicting)
            .await
            .unwrap_err();
        assert!(matches!(err, pg::ProvenanceError::Conflict(_)), "{err:?}");
    }
    let got = pg::provenance_for_runs(&client, &org.id, &ds, &["run-1", "run-2"])
        .await
        .unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(
        got["run-1"].salesforce_connection_id, connections[0],
        "first writer kept"
    );

    // One connection per source: unambiguous. A second connection delivering
    // as the same source makes every record of that source ineligible.
    assert!(
        pg::ambiguous_provenance_sources(&client, &org.id, &ds)
            .await
            .unwrap()
            .is_empty()
    );
    pg::record_provenance(&client, &row("run-3", "del-3", 1))
        .await
        .unwrap();
    let ambiguous = pg::ambiguous_provenance_sources(&client, &org.id, &ds)
        .await
        .unwrap();
    assert_eq!(ambiguous.into_iter().collect::<Vec<_>>(), ["crm"]);

    // Scoped: another org's id sees nothing.
    let none = pg::provenance_for_runs(&client, "other-org", &ds, &["run-1"])
        .await
        .unwrap();
    assert!(none.is_empty());
}
