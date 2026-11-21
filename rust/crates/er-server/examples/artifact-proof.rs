//! Synthetic local-service proof, driven by scripts/duckfleet/artifacts.py.
use std::error::Error;
use std::io::{BufRead, Write};
use std::net::SocketAddr;

use arrow::array::Int64Array;
use aws_sdk_s3::config::{Credentials, Region};
use er_duckfleet::{
    Registry,
    artifacts::{Artifact, Format, Staging},
    execution::Session,
};
use er_entities::execution::{Executor, Parameter};
use er_server::{delivery, duckfleet_artifacts::Store};
use serde_json::json;

fn main() -> Result<(), Box<dyn Error>> {
    let address: SocketAddr = std::env::var("ER_PROOF_ADDRESS")?.parse()?;
    let s3_address: SocketAddr = std::env::var("ER_PROOF_S3_ADDRESS")?.parse()?;
    assert!(address.ip().is_loopback() && s3_address.ip().is_loopback());
    let token = std::env::var("ER_PROOF_TOKEN")?;
    let scratch = tempfile::tempdir()?;
    let credentials = scratch.path().join("credentials");
    let ns = er_duckfleet::namespace("org-a", "dataset-a");
    std::fs::create_dir_all(credentials.join(&ns))?;
    std::fs::write(
        credentials.join(&ns).join("credential.json"),
        serde_json::to_vec(&json!({
            "org_id":"org-a","dataset_id":"dataset-a","generation":1,"token":token
        }))?,
    )?;
    let registry_path = scratch.path().join("registry.json");
    std::fs::write(
        &registry_path,
        serde_json::to_vec(&json!({"bucket":"duckfleet","bindings":[{
            "org_id":"org-a","dataset_id":"dataset-a","generation":1,"state":"active","valid_until":4102444800_u64
        }]}))?,
    )?;
    let registry = Registry::load(&registry_path, &credentials)?;
    let binding = registry.resolve("org-a", "dataset-a", 1, 1)?;
    let runtime = tokio::runtime::Runtime::new()?;
    let http = aws_smithy_http_client::Builder::new()
        .tls_provider(aws_smithy_http_client::tls::Provider::Rustls(
            aws_smithy_http_client::tls::rustls_provider::CryptoMode::Ring,
        ))
        .build_https();
    let config = aws_sdk_s3::config::Builder::new()
        .behavior_version_latest()
        .region(Region::new("us-east-1"))
        .endpoint_url(format!("http://{s3_address}"))
        .force_path_style(true)
        .http_client(http)
        .credentials_provider(Credentials::new(
            std::env::var("ER_PROOF_S3_KEY")?,
            std::env::var("ER_PROOF_S3_SECRET")?,
            None,
            None,
            "disposable-proof",
        ))
        .build();
    let denied_config = config
        .to_builder()
        .credentials_provider(Credentials::new(
            "expired-fixture-key",
            "expired-fixture-secret",
            None,
            None,
            "expired-fixture",
        ))
        .build();
    let denied = Store::new(aws_sdk_s3::Client::from_conf(denied_config));
    let client = aws_sdk_s3::Client::from_conf(config);
    let store = Store::new(client.clone());
    let source = er_config::load_config_str(include_str!("../../er-cli/tests/fixtures/test.yaml"))?
        .sources["crm"]
        .clone();
    let id = er_ids::new_v7();
    let path = scratch.path().join(format!(".crm.{id}.csv.part"));
    let mut file = std::io::BufWriter::new(std::fs::File::create(path)?);
    writeln!(file, "id,payload")?;
    for row in 0..60000 {
        writeln!(file, "{row},{}", "synthetic".repeat(50))?;
    }
    file.flush()?;
    drop(file);
    let pin = delivery::publish(scratch.path(), "dataset-a", "crm", &id, &source)?;
    assert!(pin.bytes > 16 * 1024 * 1024);
    let receipt = pin.params();
    let input =
        runtime.block_on(store.publish_delivery(&binding, &pin, scratch.path(), &source))?;
    let retry =
        runtime.block_on(store.publish_delivery(&binding, &pin, scratch.path(), &source))?;
    assert_eq!(input, retry);
    assert_eq!(pin.params(), receipt);
    assert!(
        runtime
            .block_on(denied.publish_delivery(&binding, &pin, scratch.path(), &source))
            .is_err()
    );
    assert_eq!(pin.params(), receipt);
    assert_eq!(
        runtime.block_on(store.publish_delivery(&binding, &pin, scratch.path(), &source))?,
        input
    );
    // A file changed after acceptance cannot silently create a new digest.
    let wrong = tempfile::NamedTempFile::new()?;
    std::fs::write(wrong.path(), b"changed")?;
    wrong.as_file().set_len(input.bytes())?;
    assert!(
        runtime
            .block_on(store.publish_file(&binding, input.clone(), wrong.reopen()?))
            .is_err()
    );
    let mut remote = Session::open_loopback_proof(address, token)?;
    remote.query(
        "CREATE TEMP TABLE artifact_input AS SELECT * FROM read_csv(?, header=true)",
        &[Parameter::Text(input.uri(&binding)?)],
    )?;
    remote.query(
        include_str!("../../er-entities/ddl/lake/raw_records.sql"),
        &[],
    )?;
    remote.query("CREATE TEMP TABLE artifact_rows AS SELECT id::VARCHAR AS id, 'fixture' AS source_system, id::VARCHAR AS source_record_id, json_object('payload', payload) AS payload, sha256(payload) AS content_hash, false AS is_deleted, NULL::TIMESTAMP AS deleted_at, 'fixture-batch' AS ingest_batch_id, TIMESTAMP '2026-01-01' AS ingested_at, TIMESTAMP '2026-01-01' AS created_at, TIMESTAMP '2026-01-01' AS updated_at FROM artifact_input", &[])?;
    remote.query("BEGIN", &[])?;
    remote.query(
        "INSERT INTO lake.main.raw_records SELECT * FROM artifact_rows",
        &[],
    )?;
    remote.query("COMMIT", &[])?;
    let count = remote.query("SELECT count(*)::BIGINT FROM lake.main.raw_records", &[])?;
    assert_eq!(
        count[0]
            .column(0)
            .as_any()
            .downcast_ref::<Int64Array>()
            .ok_or("count type")?
            .value(0),
        60000
    );
    // The worker runs with the restricted identity, and the foreign object
    // exists: neither a missing-key error nor a root credential proves isolation.
    assert!(
        remote
            .query(
                "SELECT * FROM read_csv('s3://duckfleet/foreign/artifacts/immutable/exists.csv')",
                &[]
            )
            .is_err()
    );
    let sealed_probe = Artifact::new(
        &binding,
        "worker-must-not-publish",
        Format::Csv,
        0,
        &"0".repeat(64),
    )?;
    assert!(
        remote
            .query(
                &format!(
                    "COPY (SELECT 1) TO '{}' (FORMAT CSV)",
                    sealed_probe.uri(&binding)?
                ),
                &[]
            )
            .is_err()
    );
    let staging = Staging::new(&binding, er_ids::new_v7().parse()?, Format::Csv);
    // This URI is generated from the validated binding and a UUID, never from
    // request text. COPY's target is syntax rather than a bound SQL value.
    remote.query(&format!("COPY (SELECT id::BIGINT AS id, payload->>'payload' AS payload FROM lake.main.raw_records ORDER BY id::BIGINT) TO '{}' (HEADER, FORMAT CSV)", staging.uri(&binding)?), &[])?;
    let output = runtime.block_on(store.seal(&binding, &staging, "result-1"))?;
    assert!(output.bytes() > 16 * 1024 * 1024);
    let result = runtime.block_on(store.fetch(&binding, &output))?;
    let mut lines = 0;
    for line in std::io::BufReader::new(result.reopen()?).lines() {
        line?;
        lines += 1;
    }
    assert_eq!(lines, 60001);
    assert_eq!(
        runtime.block_on(store.seal(&binding, &staging, "result-1"))?,
        output
    );
    runtime.block_on(store.remove_staging(&binding, &staging))?;
    assert!(
        runtime
            .block_on(store.seal(&binding, &staging, "result-1"))
            .is_err()
    );
    runtime.block_on(store.verify(&binding, &input))?;
    runtime.block_on(store.verify(&binding, &output))?;
    assert_eq!(
        remote.query(
            "SELECT payload->>'payload' AS payload FROM lake.main.raw_records LIMIT 1",
            &[]
        )?[0]
            .num_rows(),
        1
    );
    let bounded = remote.query(
        "SELECT payload->>'payload' AS payload FROM lake.main.raw_records",
        &[],
    );
    assert!(bounded.is_err(), "oversized SQL result must be refused");
    let bounded_kind = format!("{:?}", bounded.unwrap_err().kind);
    remote.close()?;
    let _: Artifact = serde_json::from_slice(&serde_json::to_vec(&output)?)?;
    println!(
        "{}",
        json!({"rows":60000,"worker_foreign_read_refused":true,"worker_sealed_write_refused":true,"input_bytes":input.bytes(),"output_bytes":output.bytes(),"output_sha256":output.sha256(),"delivery_retry_identical":true,"receipt_unchanged":true,"invalid_credentials_refused_retry_preserved":true,"changed_input_refused":true,"output_retry_identical":true,"staging_cleanup_preserved_sealed":true,"oversized_sql_refused":bounded_kind})
    );
    Ok(())
}
