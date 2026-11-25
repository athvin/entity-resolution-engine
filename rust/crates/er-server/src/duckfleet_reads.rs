//! Operator-only pilot selection for the golden-record page. Enabling this
//! process-wide switch requires a current binding for every admitted dataset;
//! a missing or revoked binding never falls back to embedded compute.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use er_duckfleet::execution::Session;
use er_entities::execution::Executor;

#[derive(Clone)]
pub(crate) struct Config {
    registry: PathBuf,
    credentials: PathBuf,
}

impl Config {
    pub(crate) fn from_lookup(
        lookup: impl Fn(&str) -> Option<String>,
    ) -> Option<Result<Self, String>> {
        let registry = lookup("ER_DUCKFLEET_READ_REGISTRY");
        let credentials = lookup("ER_DUCKFLEET_CREDENTIALS_DIR");
        if registry.is_none() && credentials.is_none() {
            return None;
        }
        Some(match (registry, credentials) {
            (Some(registry), Some(credentials))
                if PathBuf::from(&registry).is_absolute()
                    && PathBuf::from(&credentials).is_absolute() =>
            {
                Ok(Self {
                    registry: registry.into(),
                    credentials: credentials.into(),
                })
            }
            _ => Err("DuckFleet reads require absolute registry and credential paths".to_owned()),
        })
    }

    pub(crate) fn open(&self, org: &str, dataset: &str) -> Result<Session, String> {
        let registry = er_duckfleet::Registry::load(&self.registry, &self.credentials)
            .map_err(|error| error.to_string())?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| "invalid system clock")?
            .as_secs();
        let binding = registry
            .resolve_current(org, dataset, now)
            .map_err(|error| error.to_string())?;
        Session::open_remote(&binding).map_err(|error| error.to_string())
    }
}

/// The remote lake's `(oldest, latest)` retained snapshot versions; see
/// `dedupe_core::Session::snapshot_bounds`.
pub(crate) fn snapshot_bounds(session: &Session) -> Result<(i64, i64), String> {
    use arrow::array::{Array, Int64Array};
    // Catalog introspection; the lake alias is fixed and nothing comes from
    // the request.
    let batches = session
        .query(
            "SELECT min(snapshot_id), max(snapshot_id) FROM ducklake_snapshots('lake')",
            &[],
        )
        .map_err(|error| error.to_string())?;
    let value = |column: usize| {
        batches
            .first()
            .filter(|batch| batch.num_columns() == 2 && batch.num_rows() == 1)
            .and_then(|batch| batch.column(column).as_any().downcast_ref::<Int64Array>())
            .filter(|array| !array.is_null(0))
            .map(|array| array.value(0))
            .ok_or_else(|| "invalid snapshot response".to_owned())
    };
    Ok((value(0)?, value(1)?))
}

pub(crate) fn page_start(
    session: &Session,
    cursor: Option<&str>,
) -> Result<(i64, Option<String>), crate::read::PageStartError> {
    use arrow::array::{Array, Int64Array};
    if let Some(cursor) = cursor {
        let cursor =
            crate::read::decode_cursor(cursor).ok_or(crate::read::PageStartError::BadCursor)?;
        return Ok((cursor.snapshot, Some(cursor.key)));
    }
    // Catalog introspection, matching dedupe_core::Session::latest_snapshot_version.
    // The lake alias is fixed; no identifier or value comes from the request.
    let batches = session
        .query(
            "SELECT max(snapshot_id) FROM ducklake_snapshots('lake')",
            &[],
        )
        .map_err(|error| crate::read::PageStartError::Lake(error.to_string()))?;
    let snapshot = batches
        .first()
        .filter(|batch| batch.num_columns() == 1 && batch.num_rows() == 1)
        .and_then(|batch| batch.column(0).as_any().downcast_ref::<Int64Array>())
        .filter(|array| !array.is_null(0))
        .map(|array| array.value(0))
        .ok_or_else(|| crate::read::PageStartError::Lake("invalid snapshot response".to_owned()))?;
    Ok((snapshot, None))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_is_off_by_default_and_partial_configuration_fails_closed() {
        assert!(Config::from_lookup(|_| None).is_none());
        assert!(
            Config::from_lookup(
                |name| (name == "ER_DUCKFLEET_READ_REGISTRY").then(|| "/registry.json".to_owned())
            )
            .unwrap()
            .is_err()
        );
        assert!(
            Config::from_lookup(|_| Some("relative".to_owned()))
                .unwrap()
                .is_err()
        );
        let config = Config::from_lookup(|_| Some("/absent-duckfleet-test-path".to_owned()))
            .unwrap()
            .unwrap();
        assert!(config.open("org-a", "dataset-a").is_err());
    }
}
