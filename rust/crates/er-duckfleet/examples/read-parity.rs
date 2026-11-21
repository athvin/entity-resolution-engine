//! Real-service proof, run only by scripts/duckfleet/read-parity.py against its
//! disposable Compose project. All fixture data is synthetic.
use std::error::Error;
use std::net::SocketAddr;
use std::time::Duration;

use arrow::array::{Float64Array, Int64Array};
use arrow::compute::concat_batches;
use arrow::record_batch::RecordBatch;
use er_duckfleet::execution::{Failure, Session};
use er_entities::execution::{Executor, Parameter};
use serde_json::json;

fn scalar(session: &Session, sql: &str) -> Result<i64, Box<dyn Error>> {
    let batches = session.query(sql, &[])?;
    Ok(batches[0]
        .column(0)
        .as_any()
        .downcast_ref::<Int64Array>()
        .ok_or("not an integer")?
        .value(0))
}

fn equal(left: &[RecordBatch], right: &[RecordBatch]) -> Result<(), Box<dyn Error>> {
    let left = concat_batches(&left[0].schema(), left)?;
    let right = concat_batches(&right[0].schema(), right)?;
    assert_eq!(left.num_rows(), right.num_rows());
    assert_eq!(left.num_columns(), right.num_columns());
    for index in 0..left.num_columns() {
        assert_eq!(
            left.schema().field(index).name(),
            right.schema().field(index).name()
        );
        assert_eq!(
            left.column(index).data_type(),
            right.column(index).data_type()
        );
        assert_eq!(left.column(index).to_data(), right.column(index).to_data());
    }
    Ok(())
}

fn seed(session: &Session) -> Result<i64, Box<dyn Error>> {
    session.query(
        include_str!("../../er-entities/ddl/lake/golden_records.sql"),
        &[],
    )?;
    session.query("INSERT INTO lake.main.golden_records (id,given_name,family_name,email,birth_date,metadata,survivorship_version,assembled_at,created_at,updated_at) VALUES ('a','O''Neil','東京',NULL,DATE '2000-02-29','{\"synthetic\":true}','v1',TIMESTAMP '2026-01-02 03:04:05.123456',TIMESTAMP '2026-01-02 03:04:05',TIMESTAMP '2026-01-02 03:04:05'), ('b',NULL,'Second','synthetic@example.invalid',NULL,NULL,'v1',TIMESTAMP '2026-01-02 03:04:05',TIMESTAMP '2026-01-02 03:04:05',TIMESTAMP '2026-01-02 03:04:05')", &[])?;
    let snapshot = scalar(
        session,
        "SELECT max(snapshot_id) FROM ducklake_snapshots('lake')",
    )?;
    session.query(
        "UPDATE lake.main.golden_records SET given_name = 'later' WHERE id = 'a'",
        &[],
    )?;
    Ok(snapshot)
}

fn lifecycle(session: &Session) -> Result<(), Box<dyn Error>> {
    session.query("CREATE TEMP TABLE er_read_state (n BIGINT)", &[])?;
    session.query("BEGIN", &[])?;
    session.query(
        "INSERT INTO er_read_state VALUES (?)",
        &[Parameter::Bigint(42)],
    )?;
    assert_eq!(scalar(session, "SELECT n FROM er_read_state")?, 42);
    session.query("ROLLBACK", &[])?;
    assert_eq!(scalar(session, "SELECT count(*) FROM er_read_state")?, 0);
    assert!(session.query("SELECT missing_column", &[]).is_err());
    assert_eq!(scalar(session, "SELECT 7::BIGINT")?, 7);
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    let address: SocketAddr = std::env::var("ER_PROOF_ADDRESS")?.parse()?;
    let token = std::env::var("ER_PROOF_TOKEN")?;
    let directory = std::path::PathBuf::from(std::env::var("ER_PROOF_DIRECTORY")?);
    let connection = duckdb::Connection::open_in_memory()?;
    connection.execute_batch("INSTALL ducklake; LOAD ducklake; SET threads=1;")?;
    let catalog = directory
        .join("read-parity.ducklake")
        .to_string_lossy()
        .replace('\'', "''");
    connection.execute_batch(&format!(
        "ATTACH 'ducklake:{catalog}' AS lake (AUTOMATIC_MIGRATION false)"
    ))?;
    let mut local = Session::embedded(connection);
    let mut remote = Session::open_loopback_proof(address, token.clone())?;
    let local_snapshot = seed(&local)?;
    let remote_snapshot = seed(&remote)?;
    for (after, query, limit) in [
        (None, None, 50),
        (Some("a"), None, 1),
        (None, Some("%O'Neil%"), 50),
    ] {
        let left = er_entities::golden_record::read::list_page_with_executor(
            &local,
            local_snapshot,
            after,
            query,
            limit,
        )?;
        let right = er_entities::golden_record::read::list_page_with_executor(
            &remote,
            remote_snapshot,
            after,
            query,
            limit,
        )?;
        equal(&left, &right)?;
    }
    let old = er_entities::golden_record::read::list_page_with_executor(
        &remote,
        remote_snapshot,
        None,
        None,
        50,
    )?;
    let latest = scalar(
        &remote,
        "SELECT max(snapshot_id) FROM ducklake_snapshots('lake')",
    )?;
    let new =
        er_entities::golden_record::read::list_page_with_executor(&remote, latest, None, None, 50)?;
    assert_ne!(old[0].column(1).to_data(), new[0].column(1).to_data());
    let parameters = [
        Parameter::Bigint(i64::MAX),
        Parameter::Double(0.0012033694344163659),
        Parameter::Boolean(true),
        Parameter::Null,
        Parameter::Text("123456789012345678.90".into()),
    ];
    let sql = "SELECT $1::BIGINT AS n, $2::DOUBLE AS score, $3::BOOLEAN AS flag, $4::VARCHAR AS missing, $5::DECIMAL(20,2) AS precise";
    let left = local.query(sql, &parameters)?;
    let right = remote.query(sql, &parameters)?;
    equal(&left, &right)?;
    assert_eq!(
        right[0]
            .column(1)
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap()
            .value(0)
            .to_bits(),
        0.0012033694344163659_f64.to_bits()
    );
    lifecycle(&local)?;
    lifecycle(&remote)?;
    local.close()?;
    remote.close()?;
    assert_eq!(
        remote.query("SELECT 1", &[]).unwrap_err().kind,
        Failure::Closed
    );

    let session = Session::open_loopback_proof(address, token)?;
    let control = session.cancellation()?;
    let running = std::thread::spawn(move || {
        let result = session.query("SELECT sum(sqrt(i)) FROM range(1000000000000) AS t(i)", &[]);
        (session, result)
    });
    std::thread::sleep(Duration::from_millis(500));
    control.cancel()?;
    let (mut session, result) = running.join().map_err(|_| "query thread panicked")?;
    assert_eq!(result.unwrap_err().kind, Failure::Cancelled);
    session.close()?;
    println!(
        "{}",
        json!({"generated_snapshot_read":true,"typed_null_value_parity":true,"float_bits_preserved":true,"snapshot_isolation":true,"temporary_state_and_rollback":true,"sql_error_recovery":true,"cancellation":true,"explicit_close":true,"local_snapshot":local_snapshot,"remote_snapshot":remote_snapshot})
    );
    Ok(())
}
