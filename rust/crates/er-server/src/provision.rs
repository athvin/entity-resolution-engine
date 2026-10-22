//! Tenant auto-provisioning: what `POST /v1/orgs` does when given only a name.
//!
//! This existed in the Python control plane and was never ported, which is why
//! `create_org`'s auto branch answered a flat 503 and why six `ERSERVER_*`
//! template variables in `frontend/dev/env.sh` were read by nothing. The whole
//! contract around it survived the port intact, though — the `orgs.state` enum
//! still declares `provisioning`, `publish_config_version` still refuses a
//! provisioning org, the BFF still posts `{name}` to `/api/admin/orgs` and polls
//! a progress panel, the registry still has a `provision_job_id` column, and the
//! Playwright mock still implements the full flow. Only the server was missing.
//!
//! **Two modes, because the lake lives in different places.** A dev stack keeps
//! its DuckLake as a catalog file beside a data directory (what every `lake_e2e`
//! case uses, and what `frontend/dev/` now runs); AWS keeps the catalog in
//! Postgres and the data in S3. Provisioning has to be able to produce either,
//! so the target is a declared profile rather than a hardcoded assumption.
//!
//! **Unconfigured stays a 503.** With no `ER_PROVISION_MODE` the auto branch
//! behaves exactly as before, which is what keeps `pg_orgs`'s "the faithful 503"
//! assertion meaningful: it now tests the *unconfigured* path rather than an
//! unimplemented one.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

/// Where a provisioned tenant's lake is created.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// A DuckLake catalog FILE plus a data directory under the profile root.
    /// Single-writer by nature, which suits one dispatcher per tenant.
    Local,
    /// A per-tenant Postgres database for the catalog (`er_t_<org>`, the naming
    /// the retired compose file documented) plus an object-store data prefix.
    Postgres,
}

impl Mode {
    fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "local" => Some(Self::Local),
            "postgres" => Some(Self::Postgres),
            _ => None,
        }
    }
}

/// The operator-declared provisioning target.
#[derive(Debug, Clone)]
pub struct Profile {
    pub mode: Mode,
    /// Root for per-tenant directories: the lake (local mode), the config file
    /// and the drop directory (both modes).
    pub root: PathBuf,
    /// The starter config copied in as the tenant's version 1.
    ///
    /// Required rather than embedded. A config this server invented would be a
    /// guess at the tenant's schema that still had to satisfy
    /// `config_reference_problems`, and getting it wrong would strand a tenant
    /// with a config they cannot run. The operator states it.
    pub config_template: PathBuf,
    /// `postgres` mode: the maintenance DSN used to `CREATE DATABASE`.
    pub maint_dsn: Option<String>,
    /// `postgres` mode: the data-path template, `{org}` substituted
    /// (e.g. `s3://lake/{org}/`).
    pub data_template: Option<String>,
    /// Extra per-tenant env merged into every provisioned org — S3 credentials,
    /// DuckDB knobs, anything the engine needs that is environment rather than
    /// tenant. Declared values never override the paths derived below.
    pub env_extra: Map<String, Value>,
}

/// Why a profile could not be built. Each is an operator misconfiguration, so
/// each says which variable is wrong rather than just "invalid".
#[derive(Debug, PartialEq, Eq)]
pub enum ProfileError {
    UnknownMode(String),
    Missing(&'static str),
    BadEnvJson(String),
}

impl std::fmt::Display for ProfileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownMode(got) => {
                write!(
                    f,
                    "ER_PROVISION_MODE must be 'local' or 'postgres', got {got:?}"
                )
            }
            Self::Missing(key) => write!(f, "{key} is required when ER_PROVISION_MODE is set"),
            Self::BadEnvJson(e) => write!(f, "ER_PROVISION_ENV_JSON is not a JSON object: {e}"),
        }
    }
}

impl Profile {
    /// Read the profile from the environment. `Ok(None)` means provisioning is
    /// deliberately off — the auto branch keeps answering 503 — while `Err`
    /// means it was switched on and configured wrongly, which is worth saying
    /// out loud rather than silently treating as off.
    pub fn from_env() -> Result<Option<Self>, ProfileError> {
        let Some(raw) = std::env::var("ER_PROVISION_MODE")
            .ok()
            .filter(|v| !v.trim().is_empty())
        else {
            return Ok(None);
        };
        let mode = Mode::parse(&raw).ok_or_else(|| ProfileError::UnknownMode(raw.clone()))?;
        let required = |key: &'static str| -> Result<String, ProfileError> {
            std::env::var(key)
                .ok()
                .filter(|v| !v.trim().is_empty())
                .ok_or(ProfileError::Missing(key))
        };
        let env_extra = match std::env::var("ER_PROVISION_ENV_JSON").ok().as_deref() {
            None | Some("") => Map::new(),
            Some(raw) => match serde_json::from_str::<Value>(raw) {
                Ok(Value::Object(map)) => map,
                Ok(other) => {
                    return Err(ProfileError::BadEnvJson(format!(
                        "expected an object, got {}",
                        kind_of(&other)
                    )));
                }
                Err(e) => return Err(ProfileError::BadEnvJson(e.to_string())),
            },
        };
        Ok(Some(Self {
            mode,
            root: PathBuf::from(required("ER_PROVISION_ROOT")?),
            config_template: PathBuf::from(required("ER_PROVISION_CONFIG_TEMPLATE")?),
            maint_dsn: match mode {
                Mode::Postgres => Some(required("ER_PROVISION_MAINT_DSN")?),
                Mode::Local => None,
            },
            data_template: match mode {
                Mode::Postgres => Some(required("ER_PROVISION_DATA_TEMPLATE")?),
                Mode::Local => None,
            },
            env_extra,
        }))
    }

    /// The paths and env a tenant will get. PURE — no filesystem, no database —
    /// so `create_org` can answer with the plan while the `provision` job does
    /// the slow work, and so the derivation is testable without a lake.
    #[must_use]
    pub fn plan(&self, org: &str) -> Plan {
        let dir = self.root.join(org);
        let config_path = dir.join("config.yaml");
        let drop_root = dir.join("drop");
        let (catalog, data_path, tenant) = match self.mode {
            Mode::Local => (
                path_string(&dir.join("lake").join("catalog.ducklake")),
                path_string(&dir.join("lake").join("data")),
                None,
            ),
            Mode::Postgres => {
                let database = tenant_database(org);
                (
                    // DuckLake's Postgres catalog DSN, pointed at the tenant's
                    // own database on the same server as the maintenance DSN.
                    catalog_dsn(self.maint_dsn.as_deref().unwrap_or_default(), &database),
                    self.data_template
                        .as_deref()
                        .unwrap_or_default()
                        .replace("{org}", org),
                    Some(database),
                )
            }
        };

        // Derived paths win over `env_extra`: an operator template that also set
        // ER_LAKE_CATALOG would otherwise silently point every tenant at one
        // lake, which is a tenant-isolation failure rather than a typo.
        let mut env: Map<String, Value> = self.env_extra.clone();
        for (key, value) in [
            ("ER_CONFIG_PATH", path_string(&config_path)),
            ("ER_LAKE_CATALOG", catalog),
            ("ER_LAKE_DATA_PATH", data_path),
        ] {
            env.insert(key.to_string(), json!(value));
        }

        Plan {
            org: org.to_string(),
            dir,
            config_path,
            drop_root,
            tenant,
            env,
        }
    }
}

/// What provisioning one tenant will create.
#[derive(Debug, Clone)]
pub struct Plan {
    pub org: String,
    pub dir: PathBuf,
    pub config_path: PathBuf,
    pub drop_root: PathBuf,
    /// `postgres` mode: the catalog database to create. `None` in local mode,
    /// and the field the API reports as `tenant`.
    pub tenant: Option<String>,
    pub env: Map<String, Value>,
}

impl Plan {
    /// The org env as the `orgs.env` jsonb column.
    #[must_use]
    pub fn env_json(&self) -> Value {
        Value::Object(self.env.clone())
    }
}

/// `er_t_<org>`, with anything not legal in an unquoted identifier folded to
/// `_`. The naming the retired dev compose documented ("tenant databases
/// (er_t_*) are created here by provisioning").
#[must_use]
pub fn tenant_database(org: &str) -> String {
    let safe: String = org
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    format!("er_t_{safe}")
}

/// Re-point a Postgres DSN at another database, keeping host, credentials and
/// query string. Used to turn the maintenance DSN into the tenant's catalog DSN.
#[must_use]
fn catalog_dsn(maint: &str, database: &str) -> String {
    // Split off any query string, replace the last path segment, reassemble.
    let (base, query) = match maint.split_once('?') {
        Some((b, q)) => (b, Some(q)),
        None => (maint, None),
    };
    let trimmed = base.trim_end_matches('/');
    let replaced = match trimmed.rfind('/') {
        // Only treat the tail as a database name when it is not part of the
        // scheme (`postgresql://host` has its last '/' inside the scheme).
        Some(idx) if !trimmed[..idx].ends_with(':') && !trimmed[..idx].ends_with('/') => {
            format!("{}/{database}", &trimmed[..idx])
        }
        _ => format!("{trimmed}/{database}"),
    };
    match query {
        Some(q) => format!("{replaced}?{q}"),
        None => replaced,
    }
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn kind_of(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

/// Create the tenant's directories and starter config. Idempotent: a retried
/// `provision` job must not fail because the first attempt got this far.
///
/// # Errors
/// Returns the IO failure, named with the path it was trying to create.
pub fn create_workspace(plan: &Plan, template: &Path) -> Result<(), String> {
    for dir in [&plan.dir, &plan.drop_root] {
        std::fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    }
    let yaml = std::fs::read_to_string(template)
        .map_err(|e| format!("read starter config {}: {e}", template.display()))?;
    // Only write the config when absent, so a retry cannot clobber a config the
    // tenant has since published over.
    if !plan.config_path.exists() {
        std::fs::write(&plan.config_path, yaml)
            .map_err(|e| format!("write {}: {e}", plan.config_path.display()))?;
    }
    Ok(())
}

/// The starter config's text, for recording as the tenant's config version 1.
///
/// # Errors
/// Returns the read failure.
pub fn starter_config(template: &Path) -> Result<String, String> {
    std::fs::read_to_string(template)
        .map_err(|e| format!("read starter config {}: {e}", template.display()))
}

/// Create the local DuckLake: the lake directory and its data subdirectory. The
/// catalog FILE is created by DuckDB on first attach, so this only has to make
/// the places it will live. Idempotent.
///
/// # Errors
/// Returns the IO failure.
pub fn create_local_lake(plan: &Plan) -> Result<(), String> {
    let catalog = plan
        .env
        .get("ER_LAKE_CATALOG")
        .and_then(Value::as_str)
        .ok_or("plan is missing ER_LAKE_CATALOG")?;
    let data = plan
        .env
        .get("ER_LAKE_DATA_PATH")
        .and_then(Value::as_str)
        .ok_or("plan is missing ER_LAKE_DATA_PATH")?;
    if let Some(parent) = Path::new(catalog).parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
    }
    std::fs::create_dir_all(data).map_err(|e| format!("create {data}: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(mode: Mode) -> Profile {
        Profile {
            mode,
            root: PathBuf::from("/srv/tenants"),
            config_template: PathBuf::from("/srv/starter.yaml"),
            maint_dsn: Some("postgresql://postgres:pw@catalog:5432/ducklake".to_string()),
            data_template: Some("s3://lake/{org}/".to_string()),
            env_extra: Map::new(),
        }
    }

    #[test]
    fn a_local_plan_puts_the_lake_under_the_tenants_own_directory() {
        let plan = profile(Mode::Local).plan("acme");
        assert_eq!(
            plan.env["ER_CONFIG_PATH"],
            json!("/srv/tenants/acme/config.yaml")
        );
        assert_eq!(
            plan.env["ER_LAKE_CATALOG"],
            json!("/srv/tenants/acme/lake/catalog.ducklake")
        );
        assert_eq!(
            plan.env["ER_LAKE_DATA_PATH"],
            json!("/srv/tenants/acme/lake/data")
        );
        assert_eq!(plan.drop_root, PathBuf::from("/srv/tenants/acme/drop"));
        // No catalog database in local mode; the API reports no `tenant`.
        assert!(plan.tenant.is_none());
    }

    #[test]
    fn a_postgres_plan_gets_its_own_catalog_database_and_data_prefix() {
        let plan = profile(Mode::Postgres).plan("acme");
        assert_eq!(plan.tenant.as_deref(), Some("er_t_acme"));
        // The catalog DSN keeps host and credentials and swaps only the database,
        // so a tenant cannot end up sharing the maintenance database.
        assert_eq!(
            plan.env["ER_LAKE_CATALOG"],
            json!("postgresql://postgres:pw@catalog:5432/er_t_acme")
        );
        assert_eq!(plan.env["ER_LAKE_DATA_PATH"], json!("s3://lake/acme/"));
    }

    /// Two tenants must never share a lake. This is the assertion that would
    /// catch a template substitution silently dropping `{org}`.
    #[test]
    fn two_tenants_never_share_a_catalog_or_a_data_path() {
        for mode in [Mode::Local, Mode::Postgres] {
            let a = profile(mode).plan("alpha");
            let b = profile(mode).plan("beta");
            assert_ne!(
                a.env["ER_LAKE_CATALOG"], b.env["ER_LAKE_CATALOG"],
                "{mode:?}"
            );
            assert_ne!(
                a.env["ER_LAKE_DATA_PATH"], b.env["ER_LAKE_DATA_PATH"],
                "{mode:?}"
            );
            assert_ne!(a.config_path, b.config_path, "{mode:?}");
            assert_ne!(a.drop_root, b.drop_root, "{mode:?}");
        }
    }

    /// An operator template that also names a lake must not win. Letting it
    /// through would point every tenant at one catalog — a tenant-isolation
    /// failure dressed as a configuration typo.
    #[test]
    fn declared_env_cannot_override_the_derived_lake_paths() {
        let mut p = profile(Mode::Local);
        p.env_extra = serde_json::from_str(
            r#"{"ER_LAKE_CATALOG":"/shared/everyones.ducklake",
                "ER_LAKE_DATA_PATH":"/shared/data",
                "ER_CONFIG_PATH":"/shared/config.yaml",
                "ER_DUCKDB_THREADS":"4"}"#,
        )
        .unwrap();
        let plan = p.plan("acme");
        assert_eq!(
            plan.env["ER_LAKE_CATALOG"],
            json!("/srv/tenants/acme/lake/catalog.ducklake")
        );
        assert_eq!(
            plan.env["ER_LAKE_DATA_PATH"],
            json!("/srv/tenants/acme/lake/data")
        );
        assert_eq!(
            plan.env["ER_CONFIG_PATH"],
            json!("/srv/tenants/acme/config.yaml")
        );
        // Everything else the operator declared is kept.
        assert_eq!(plan.env["ER_DUCKDB_THREADS"], json!("4"));
    }

    #[test]
    fn a_tenant_database_name_is_a_legal_identifier() {
        assert_eq!(tenant_database("acme"), "er_t_acme");
        assert_eq!(tenant_database("ACME-Corp"), "er_t_acme_corp");
        assert_eq!(tenant_database("a.b c"), "er_t_a_b_c");
    }

    #[test]
    fn a_catalog_dsn_swaps_only_the_database() {
        assert_eq!(
            catalog_dsn("postgresql://u:p@h:5432/ducklake", "er_t_x"),
            "postgresql://u:p@h:5432/er_t_x"
        );
        // A trailing slash is not a database name.
        assert_eq!(
            catalog_dsn("postgresql://u:p@h:5432/", "er_t_x"),
            "postgresql://u:p@h:5432/er_t_x"
        );
        // No database at all: append rather than eat the host.
        assert_eq!(
            catalog_dsn("postgresql://u:p@h:5432", "er_t_x"),
            "postgresql://u:p@h:5432/er_t_x"
        );
        // Query parameters (sslmode and friends) survive.
        assert_eq!(
            catalog_dsn("postgresql://u:p@h/ducklake?sslmode=require", "er_t_x"),
            "postgresql://u:p@h/er_t_x?sslmode=require"
        );
    }
}
