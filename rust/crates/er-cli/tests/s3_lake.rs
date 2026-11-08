//! The proving test for the object-store credential path (#792): an `s3://`
//! lake attaches, writes and reads through a session-scoped S3 secret, against
//! a real S3-compatible store. Two gates, both the `ER_TEST_PG_DSN` self-skip
//! idiom:
//!
//! - `explicit_key_opens_an_s3_lake` needs `ER_TEST_S3_ENDPOINT` +
//!   `ER_TEST_S3_KEY_ID` + `ER_TEST_S3_SECRET` (the MinIO the CI leg runs;
//!   `ER_TEST_S3_BUCKET` names the bucket, default `er-lake-e2e`).
//! - `empty_config_resolves_the_process_env` needs the `ER_LAKE_S3_*` set in
//!   the process env — it asserts the attach-time fallback itself, so the
//!   values may not come from a config the test builds. `std::env::set_var`
//!   is unsafe and the workspace denies `unsafe_code`, so the CI leg sets
//!   them job-wide (pointing at the same MinIO) and the local recipe exports
//!   them — the same seam `pg_provision` and `executor_harness` document.
//!
//! Locally:
//! ```sh
//! docker run -d --name er-minio -p 9000:9000 \
//!   -e MINIO_ROOT_USER=minio -e MINIO_ROOT_PASSWORD=minio123 \
//!   minio/minio server /data
//! docker run --rm --network host minio/mc sh -c \
//!   'mc alias set local http://localhost:9000 minio minio123 && mc mb --ignore-existing local/er-lake-e2e'
//! export ER_TEST_S3_ENDPOINT=localhost:9000 ER_TEST_S3_KEY_ID=minio ER_TEST_S3_SECRET=minio123
//! export ER_LAKE_S3_ENDPOINT=localhost:9000 ER_LAKE_S3_KEY_ID=minio ER_LAKE_S3_SECRET=minio123 \
//!        ER_LAKE_S3_URL_STYLE=path ER_LAKE_S3_USE_SSL=false
//! cargo test -p er-cli --test s3_lake
//! ```

use dedupe_core::session::SessionOptions;
use dedupe_core::session::ducklake::{DuckLakeConfig, ObjectStoreConfig};
use dedupe_core::session::{Session, TableHandle};

struct Minio {
    endpoint: String,
    key_id: String,
    secret: String,
    bucket: String,
}

fn minio() -> Option<Minio> {
    let endpoint = std::env::var("ER_TEST_S3_ENDPOINT")
        .ok()
        .filter(|v| !v.is_empty())?;
    let key_id = std::env::var("ER_TEST_S3_KEY_ID")
        .ok()
        .filter(|v| !v.is_empty())?;
    let secret = std::env::var("ER_TEST_S3_SECRET")
        .ok()
        .filter(|v| !v.is_empty())?;
    Some(Minio {
        endpoint,
        key_id,
        secret,
        bucket: bucket(),
    })
}

fn bucket() -> String {
    std::env::var("ER_TEST_S3_BUCKET")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "er-lake-e2e".to_string())
}

/// Attach a fresh session to a lake whose catalog is a temp FILE (the
/// metadata half is unchanged by this feature) and whose DATA_PATH is the
/// test bucket under a per-test prefix.
fn attach(bucket: &str, tag: &str, object_store: ObjectStoreConfig, read_only: bool) -> Session {
    let tmp = std::env::temp_dir().join(format!("er_s3_lake_{tag}_{}", std::process::id()));
    if !read_only {
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
    }
    let mut session = Session::open_in_memory(SessionOptions::deterministic()).unwrap();
    session
        .attach_ducklake(&DuckLakeConfig {
            metadata: tmp.join("meta.ducklake").to_string_lossy().into_owned(),
            data_path: Some(format!("s3://{bucket}/{tag}-{}/", std::process::id())),
            read_only,
            object_store,
            ..DuckLakeConfig::default()
        })
        .unwrap();
    session
}

/// Write one two-row table to the lake and read it back through the alias.
fn round_trip(session: &Session) {
    session
        .execute_batch(
            "CREATE TEMP TABLE stage AS SELECT * FROM (VALUES (1,'a'),(2,'b')) v(id,name)",
        )
        .unwrap();
    let handle = TableHandle {
        templated: "stage".into(),
        physical: "stage".into(),
        columns: vec!["id".into(), "name".into()],
    };
    session.write_to_lake(&handle, "t").unwrap();

    // `register_lake_table` is the production read idiom — and it keeps the
    // ad-hoc `t` out of the declared-names posterity gate, which scans for
    // qualified `lake.main.*` literals.
    let back = session.register_lake_table("t", "t_read").unwrap();
    let rows: Vec<(i32, String)> = session
        .connection()
        .prepare(&format!(
            "select id, name from {} order by id",
            back.physical
        ))
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(rows, vec![(1, "a".to_string()), (2, "b".to_string())]);
}

/// The acceptance criterion: an `s3://` lake opens from Rust, with a static
/// key (the MinIO/CI shape) — write in one session, read back in a FRESH one
/// (so the data provably crossed the object store, not a shared catalog).
#[test]
fn explicit_key_opens_an_s3_lake() {
    let Some(m) = minio() else {
        eprintln!("skipping: ER_TEST_S3_ENDPOINT/KEY_ID/SECRET unset (see the header)");
        return;
    };
    let config = || ObjectStoreConfig {
        key_id: Some(m.key_id.clone()),
        secret: Some(m.secret.clone()),
        endpoint: Some(m.endpoint.clone()),
        url_style: Some("path".to_string()),
        use_ssl: Some(false),
        ..ObjectStoreConfig::default()
    };
    round_trip(&attach(&m.bucket, "explicit", config(), false));
    let reopened = attach(&m.bucket, "explicit", config(), true);
    let back = reopened.register_lake_table("t", "t_read").unwrap();
    let count: i64 = reopened
        .connection()
        .query_row(
            &format!("select count(*) from {}", back.physical),
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 2, "the fresh read-only attach reads the s3 data");
}

/// The wiring every lake-open seam relies on: an EMPTY `object_store`
/// resolves the `ER_LAKE_S3_*` process env inside `attach_ducklake`, so no
/// seam can forget to thread the config through. The test never sees the
/// values — if the fallback stopped reading them, MinIO would refuse the
/// attach and this would be red, not green.
#[test]
fn empty_config_resolves_the_process_env() {
    if std::env::var("ER_LAKE_S3_ENDPOINT").is_err()
        || std::env::var("ER_LAKE_S3_KEY_ID").is_err()
        || std::env::var("ER_LAKE_S3_SECRET").is_err()
    {
        eprintln!("skipping: ER_LAKE_S3_ENDPOINT/KEY_ID/SECRET unset (see the header)");
        return;
    }
    round_trip(&attach(
        &bucket(),
        "env",
        ObjectStoreConfig::default(),
        false,
    ));
}
