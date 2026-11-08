//! DuckLake attachment layer (completed in milestone M7).
//!
//! Architecture: inputs are read from the attached `lake` catalog via
//! cross-catalog SELECTs; all `__dedupe__*` intermediates live in the native
//! temp catalog (DuckLake cannot host temp tables); final outputs are written
//! back to the lake in dedicated single-catalog transactions.

use crate::error::DedupeError;

#[derive(Debug, Clone)]
pub struct DuckLakeConfig {
    /// Everything after `ducklake:` in the ATTACH string, verbatim — e.g.
    /// `meta.ducklake`, `sqlite:meta.db`, `postgres:dbname=lake host=...`.
    pub metadata: String,
    /// `DATA_PATH` attach option (Parquet data location).
    pub data_path: Option<String>,
    /// `METADATA_SCHEMA` attach option: the schema inside the metadata
    /// database where DuckLake keeps its own catalog tables. Distinct lakes
    /// can share one Postgres database by each taking a schema of their own.
    /// `None` omits the option (DuckLake's default). Not for file catalogs.
    ///
    /// Not to be confused with [`schema`](Self::schema), which is where the
    /// *user tables* live inside the attached catalog.
    pub metadata_schema: Option<String>,
    /// Catalog alias; never `USE`d — all references are fully qualified.
    pub alias: String,
    /// Schema within the lake catalog.
    pub schema: String,
    pub read_only: bool,
    /// S3 configuration, consulted only when `data_path` is an `s3://` URL:
    /// the attach then emits a session-scoped `CREATE OR REPLACE SECRET`
    /// before the ATTACH. An empty config resolves the process environment
    /// at attach time (see [`ObjectStoreConfig::from_process_env`]); an
    /// explicit config always wins. Ignored for a filesystem data path.
    pub object_store: ObjectStoreConfig,
}

impl Default for DuckLakeConfig {
    fn default() -> Self {
        DuckLakeConfig {
            metadata: String::new(),
            data_path: None,
            metadata_schema: None,
            alias: "lake".to_string(),
            schema: "main".to_string(),
            read_only: false,
            object_store: ObjectStoreConfig::default(),
        }
    }
}

/// Physical layout options for a lake write ([`write_to_lake_with`]).
///
/// DuckLake stores tables as Parquet files with a SQL catalog and has **no
/// secondary indexes** (`CREATE INDEX` is rejected — tables are columnar
/// files, not B-trees). The two levers it *does* offer are partitioning and
/// sort-on-write, both captured here:
///
/// - [`partition_by`](Self::partition_by): writes one Parquet subtree per
///   distinct value (`col=val/…`), so a reader filtering on that column scans
///   only matching files. Use it for columns downstream queries filter on —
///   `source_dataset`, a batch/date column, a cluster bucket. It does **not**
///   speed up the in-engine blocking join (which runs on native temp tables,
///   not the lake).
/// - [`order_by`](Self::order_by): physically sorts rows on write so each
///   Parquet file's min/max zone-maps are tight, letting range/equality
///   filters skip whole files. This is DuckLake's equivalent of a clustering
///   key — no separate index object, just well-ordered data plus statistics.
///
/// [`write_to_lake_with`]: super::Session::write_to_lake_with
#[derive(Debug, Clone, Default)]
pub struct LakeWriteOptions {
    /// Partition columns (column names; empty = unpartitioned single table).
    pub partition_by: Vec<String>,
    /// Sort columns applied on write (empty = no explicit ordering).
    pub order_by: Vec<String>,
}

impl LakeWriteOptions {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.partition_by.is_empty() && self.order_by.is_empty()
    }
}

/// Process-env names [`ObjectStoreConfig::from_process_env`] reads. S3
/// configuration is host-level — the pod's endpoint overrides and, for
/// MinIO/CI only, a static key pair. These names are deliberately NOT the
/// `ER_S3_ACCESS_KEY_ID`/`ER_S3_SECRET_ACCESS_KEY` pair
/// `tf-static-credentials-check.sh` bans: those stay banned (a
/// credential-shaped name nothing should consume in deployed envs), so the
/// gate needs no narrowing.
///
/// Nothing credential-shaped with these names may ever reach an org row —
/// `er_server::pg`'s env-write filter refuses `*_KEY_ID`/`*_SECRET`/
/// `*_TOKEN`/`*_PASSWORD`/`*_ACCESS_KEY` keys there, because an org row is
/// projected into job params and travels further than process env.
pub const ENV_S3_KEY_ID: &str = "ER_LAKE_S3_KEY_ID";
/// See [`ENV_S3_KEY_ID`].
pub const ENV_S3_SECRET: &str = "ER_LAKE_S3_SECRET";
/// See [`ENV_S3_KEY_ID`].
pub const ENV_S3_REGION: &str = "ER_LAKE_S3_REGION";
/// See [`ENV_S3_KEY_ID`].
pub const ENV_S3_ENDPOINT: &str = "ER_LAKE_S3_ENDPOINT";
/// See [`ENV_S3_KEY_ID`].
pub const ENV_S3_URL_STYLE: &str = "ER_LAKE_S3_URL_STYLE";
/// See [`ENV_S3_KEY_ID`].
pub const ENV_S3_USE_SSL: &str = "ER_LAKE_S3_USE_SSL";

/// The fixed name of the session secret an `s3://` attach creates. Fixed
/// because `CREATE OR REPLACE` makes a re-attach idempotent, and a session
/// attaches one lake.
pub const S3_SECRET_NAME: &str = "er_lake_s3";

/// Object-store (S3) configuration for a lake whose `DATA_PATH` is an
/// `s3://` URL. Every field is optional.
///
/// Emitted as a session-scoped `CREATE OR REPLACE SECRET` ahead of the
/// ATTACH (never `PERSISTENT` — nothing lands in `~/.duckdb`). The shape of
/// the credential selects the provider — there is deliberately no mode flag,
/// because a mode flag is a second thing that can disagree with the
/// credential:
///
/// - No explicit key → `PROVIDER CREDENTIAL_CHAIN`: the environment's own
///   identity (EKS Pod Identity in-cluster, an instance profile, `AWS_*`
///   variables on a developer machine). The pod's role already carries the
///   lake-bucket grants, so the deployed path needs no secret material at
///   all.
/// - `key_id` + `secret` together → a static `KEY_ID`/`SECRET` secret, for
///   MinIO and CI, which have no chain to consult. One without the other is
///   an error: a half-specified credential must not silently fall back to
///   the chain.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ObjectStoreConfig {
    /// Static access key id (MinIO/CI only). `None` selects the credential
    /// chain.
    pub key_id: Option<String>,
    /// Static secret paired with [`key_id`](Self::key_id).
    pub secret: Option<String>,
    /// Region override; `None` leaves the SDK's own resolution
    /// (`AWS_REGION`, instance metadata).
    pub region: Option<String>,
    /// Endpoint override for S3-compatible stores — a bare `host:port`
    /// (DuckDB applies the scheme from [`use_ssl`](Self::use_ssl)).
    pub endpoint: Option<String>,
    /// `path` (MinIO) or `vhost` (the AWS default).
    pub url_style: Option<String>,
    /// `false` for a plain-http endpoint (local MinIO).
    pub use_ssl: Option<bool>,
}

impl ObjectStoreConfig {
    /// No field set. An `s3://` attach resolves the process environment in
    /// exactly this case; an explicit config always wins.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.key_id.is_none()
            && self.secret.is_none()
            && self.region.is_none()
            && self.endpoint.is_none()
            && self.url_style.is_none()
            && self.use_ssl.is_none()
    }

    /// Resolve from a lookup — the pure seam tests drive; production uses
    /// [`from_process_env`](Self::from_process_env).
    ///
    /// # Errors
    /// A half-specified static credential (exactly one of `KEY_ID`/`SECRET`
    /// set), or an `USE_SSL` value that is not a boolean.
    pub fn from_env_with(get: impl Fn(&str) -> Option<String>) -> Result<Self, DedupeError> {
        let non_empty = |name: &str| get(name).filter(|v| !v.is_empty());
        let key_id = non_empty(ENV_S3_KEY_ID);
        let secret = non_empty(ENV_S3_SECRET);
        if key_id.is_some() != secret.is_some() {
            let (set, missing) = if key_id.is_some() {
                (ENV_S3_KEY_ID, ENV_S3_SECRET)
            } else {
                (ENV_S3_SECRET, ENV_S3_KEY_ID)
            };
            return Err(DedupeError::settings(format!(
                "{set} is set without {missing}: a half-specified static S3 credential — \
                 set both or neither (neither selects the credential chain)"
            )));
        }
        let use_ssl = match non_empty(ENV_S3_USE_SSL) {
            None => None,
            Some(v) => match v.to_ascii_lowercase().as_str() {
                "true" | "1" | "yes" => Some(true),
                "false" | "0" | "no" => Some(false),
                other => {
                    return Err(DedupeError::settings(format!(
                        "{ENV_S3_USE_SSL} must be true or false (got {other:?})"
                    )));
                }
            },
        };
        Ok(ObjectStoreConfig {
            key_id,
            secret,
            region: non_empty(ENV_S3_REGION),
            endpoint: non_empty(ENV_S3_ENDPOINT),
            url_style: non_empty(ENV_S3_URL_STYLE),
            use_ssl,
        })
    }

    /// Resolve from the process environment. S3 configuration is
    /// environment-level, never per-org, so the process env is the only
    /// source any lake-open seam consults.
    ///
    /// # Errors
    /// As [`from_env_with`](Self::from_env_with).
    pub fn from_process_env() -> Result<Self, DedupeError> {
        Self::from_env_with(|name| std::env::var(name).ok())
    }

    /// The session-scoped `CREATE OR REPLACE SECRET` statement for this
    /// config: static `KEY_ID`/`SECRET` when both are present,
    /// `PROVIDER CREDENTIAL_CHAIN` otherwise.
    ///
    /// # Errors
    /// A half-specified static credential, however the config was built.
    pub fn secret_sql(&self) -> Result<String, DedupeError> {
        fn quoted(v: &str) -> String {
            v.replace('\'', "''")
        }
        let mut parts = vec!["TYPE S3".to_string()];
        match (&self.key_id, &self.secret) {
            (Some(key_id), Some(secret)) => {
                parts.push(format!("KEY_ID '{}'", quoted(key_id)));
                parts.push(format!("SECRET '{}'", quoted(secret)));
            }
            (None, None) => parts.push("PROVIDER CREDENTIAL_CHAIN".to_string()),
            _ => {
                return Err(DedupeError::settings(
                    "a half-specified static S3 credential: key_id and secret come \
                     together, or neither (neither selects the credential chain)",
                ));
            }
        }
        if let Some(region) = &self.region {
            parts.push(format!("REGION '{}'", quoted(region)));
        }
        if let Some(endpoint) = &self.endpoint {
            parts.push(format!("ENDPOINT '{}'", quoted(endpoint)));
        }
        if let Some(url_style) = &self.url_style {
            parts.push(format!("URL_STYLE '{}'", quoted(url_style)));
        }
        if let Some(use_ssl) = self.use_ssl {
            parts.push(format!("USE_SSL {use_ssl}"));
        }
        Ok(format!(
            "CREATE OR REPLACE SECRET {S3_SECRET_NAME} ({});",
            parts.join(", ")
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_config_selects_the_credential_chain() {
        let sql = ObjectStoreConfig::default().secret_sql().unwrap();
        assert_eq!(
            sql,
            "CREATE OR REPLACE SECRET er_lake_s3 (TYPE S3, PROVIDER CREDENTIAL_CHAIN);"
        );
    }

    #[test]
    fn explicit_key_emits_a_static_secret() {
        let sql = ObjectStoreConfig {
            key_id: Some("minio".to_string()),
            secret: Some("minio123".to_string()),
            endpoint: Some("localhost:9000".to_string()),
            url_style: Some("path".to_string()),
            use_ssl: Some(false),
            ..ObjectStoreConfig::default()
        }
        .secret_sql()
        .unwrap();
        assert_eq!(
            sql,
            "CREATE OR REPLACE SECRET er_lake_s3 (TYPE S3, KEY_ID 'minio', SECRET 'minio123', \
             ENDPOINT 'localhost:9000', URL_STYLE 'path', USE_SSL false);"
        );
    }

    #[test]
    fn a_half_specified_key_is_an_error_not_a_chain_fallback() {
        let err = ObjectStoreConfig {
            key_id: Some("minio".to_string()),
            ..ObjectStoreConfig::default()
        }
        .secret_sql()
        .unwrap_err();
        assert!(err.to_string().contains("half-specified"), "{err}");
    }

    #[test]
    fn quoting_survives_single_quotes() {
        let sql = ObjectStoreConfig {
            key_id: Some("o'brien".to_string()),
            secret: Some("s'ecret".to_string()),
            ..ObjectStoreConfig::default()
        }
        .secret_sql()
        .unwrap();
        assert!(sql.contains("KEY_ID 'o''brien'"), "{sql}");
        assert!(sql.contains("SECRET 's''ecret'"), "{sql}");
    }

    #[test]
    fn env_resolution_reads_the_er_lake_s3_set() {
        let vars = |name: &str| match name {
            ENV_S3_KEY_ID => Some("k".to_string()),
            ENV_S3_SECRET => Some("s".to_string()),
            ENV_S3_ENDPOINT => Some("minio:9000".to_string()),
            ENV_S3_URL_STYLE => Some("path".to_string()),
            ENV_S3_USE_SSL => Some("false".to_string()),
            _ => None,
        };
        let cfg = ObjectStoreConfig::from_env_with(vars).unwrap();
        assert_eq!(
            cfg,
            ObjectStoreConfig {
                key_id: Some("k".to_string()),
                secret: Some("s".to_string()),
                region: None,
                endpoint: Some("minio:9000".to_string()),
                url_style: Some("path".to_string()),
                use_ssl: Some(false),
            }
        );
    }

    #[test]
    fn env_resolution_refuses_a_lone_key_id() {
        let vars = |name: &str| (name == ENV_S3_KEY_ID).then(|| "k".to_string());
        let err = ObjectStoreConfig::from_env_with(vars).unwrap_err();
        assert!(
            err.to_string().contains(ENV_S3_KEY_ID) && err.to_string().contains(ENV_S3_SECRET),
            "{err}"
        );
    }

    #[test]
    fn env_resolution_refuses_a_non_boolean_use_ssl() {
        let vars = |name: &str| (name == ENV_S3_USE_SSL).then(|| "maybe".to_string());
        let err = ObjectStoreConfig::from_env_with(vars).unwrap_err();
        assert!(err.to_string().contains(ENV_S3_USE_SSL), "{err}");
    }

    #[test]
    fn empty_env_resolves_to_the_empty_config() {
        let cfg = ObjectStoreConfig::from_env_with(|_| None).unwrap();
        assert!(cfg.is_empty());
        // …and the empty config is the credential chain, not "no secret".
        assert!(cfg.secret_sql().unwrap().contains("CREDENTIAL_CHAIN"));
    }
}
