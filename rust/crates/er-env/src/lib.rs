//! The one reader of the `ER_*` environment set (port of `src/er/lake/env.py`).
//!
//! A thread-local overlay (installed by [`lake_environment`]) is consulted before
//! the process environment, so a host can substitute a tenant's lake env without
//! mutating the process env. Every error exposes `code() == 2` (config) — the
//! convention `er_errors::exit_code_for` honours — and this crate imports no
//! taxonomy. (Python uses a contextvar; the sync engine maps to a thread-local.
//! The async multi-tenant control plane will swap in a task-local in a later wave.)

use std::cell::RefCell;
use std::collections::HashMap;

/// Missing/invalid env is a config error: S4.0 exit 2.
pub const ENV_ERROR_CODE: i32 = 2;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum EnvError {
    /// The S4.0b named literal; operators and tests match on it exactly.
    #[error("ERR_ENV_MISSING: {0}")]
    Missing(String),
    #[error("ERR_ENV_INVALID: {name} is not {expected} (got {value:?})")]
    Invalid {
        name: String,
        value: String,
        expected: &'static str,
    },
}

impl EnvError {
    #[must_use]
    pub fn code(&self) -> i32 {
        ENV_ERROR_CODE
    }
    #[must_use]
    pub fn name(&self) -> &str {
        match self {
            EnvError::Missing(name) | EnvError::Invalid { name, .. } => name,
        }
    }
}

thread_local! {
    static OVERLAY: RefCell<Option<HashMap<String, String>>> = const { RefCell::new(None) };
}

/// RAII guard restoring the previous overlay on drop (the contextmanager scope).
#[must_use = "the overlay is active only while the guard is held"]
pub struct OverlayGuard {
    previous: Option<HashMap<String, String>>,
}

impl Drop for OverlayGuard {
    fn drop(&mut self) {
        OVERLAY.with(|cell| *cell.borrow_mut() = self.previous.take());
    }
}

/// Read `ER_*` from `overrides` first, for this thread until the guard drops.
/// Merge semantics (names absent from `overrides` still resolve from the process
/// env); nested overlays shadow and restore the outer one.
pub fn lake_environment<S: std::hash::BuildHasher>(
    overrides: HashMap<String, String, S>,
) -> OverlayGuard {
    OVERLAY.with(|cell| {
        let mut current = cell.borrow_mut();
        let previous = current.clone();
        let mut merged = current.clone().unwrap_or_default();
        merged.extend(overrides);
        *current = Some(merged);
        OverlayGuard { previous }
    })
}

fn lookup(name: &str) -> Option<String> {
    // Overlay wins only when it carries the name; otherwise the process env.
    let overlaid = OVERLAY.with(|cell| cell.borrow().as_ref().and_then(|m| m.get(name).cloned()));
    if overlaid.is_some() {
        return overlaid;
    }
    std::env::var(name).ok()
}

/// `$name`, erroring if absent or empty (whitespace-only counts as empty). The
/// value is returned unstripped.
pub fn require_env(name: &str) -> Result<String, EnvError> {
    match lookup(name) {
        Some(value) if !value.trim().is_empty() => Ok(value),
        _ => Err(EnvError::Missing(name.to_string())),
    }
}

/// `$name` via the same overlay-then-process resolution, or `None`.
#[must_use]
pub fn optional_env(name: &str) -> Option<String> {
    lookup(name)
}

fn is_plain_integer(s: &str) -> bool {
    let body = s.strip_prefix(['+', '-']).unwrap_or(s);
    !body.is_empty() && body.bytes().all(|b| b.is_ascii_digit())
}

/// `$name` as an int, rejecting anything not a plain (optionally-signed) integer.
pub fn require_int_env(name: &str) -> Result<i64, EnvError> {
    let value = require_env(name)?;
    let parsed = {
        let trimmed = value.trim();
        if is_plain_integer(trimmed) {
            trimmed.parse::<i64>().ok()
        } else {
            None
        }
    };
    parsed.ok_or(EnvError::Invalid {
        name: name.to_string(),
        value,
        expected: "an integer",
    })
}

/// `$name` as a bool, accepting only the SQL spellings `true`/`false`.
pub fn require_bool_env(name: &str) -> Result<bool, EnvError> {
    let value = require_env(name)?;
    match value.trim().to_ascii_lowercase().as_str() {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(EnvError::Invalid {
            name: name.to_string(),
            value,
            expected: "'true' or 'false'",
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn overlay(pairs: &[(&str, &str)]) -> OverlayGuard {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        lake_environment(map)
    }

    #[test]
    fn overlay_read_and_missing() {
        let _g = overlay(&[("ER_X", "val")]);
        assert_eq!(require_env("ER_X").unwrap(), "val");
        assert_eq!(optional_env("ER_X"), Some("val".to_string()));
        // Absent (not in overlay, not in process env) -> Missing, code 2.
        let err = require_env("ER_UNSET_ZZZ_123").unwrap_err();
        assert!(matches!(err, EnvError::Missing(_)));
        assert_eq!(err.code(), 2);
        assert_eq!(err.name(), "ER_UNSET_ZZZ_123");
    }

    #[test]
    fn whitespace_only_is_empty() {
        let _g = overlay(&[("ER_WS", "   ")]);
        assert!(matches!(require_env("ER_WS"), Err(EnvError::Missing(_))));
        assert_eq!(optional_env("ER_WS"), Some("   ".to_string())); // optional does not check
    }

    #[test]
    fn int_and_bool_parsing() {
        let _g = overlay(&[
            ("ER_INT", "6"),
            ("ER_NEG", "-3"),
            ("ER_BADINT", "2; DROP"),
            ("ER_BOOL_F", "false"),
            ("ER_BOOL_T", "TRUE"),
            ("ER_BADBOOL", "yes"),
        ]);
        assert_eq!(require_int_env("ER_INT").unwrap(), 6);
        assert_eq!(require_int_env("ER_NEG").unwrap(), -3);
        assert!(matches!(
            require_int_env("ER_BADINT"),
            Err(EnvError::Invalid { .. })
        ));
        assert!(!require_bool_env("ER_BOOL_F").unwrap());
        assert!(require_bool_env("ER_BOOL_T").unwrap());
        assert!(matches!(
            require_bool_env("ER_BADBOOL"),
            Err(EnvError::Invalid { .. })
        ));
    }

    #[test]
    fn overlay_nests_and_restores() {
        {
            let _outer = overlay(&[("ER_A", "1")]);
            assert_eq!(require_env("ER_A").unwrap(), "1");
            {
                let _inner = overlay(&[("ER_A", "2")]);
                assert_eq!(require_env("ER_A").unwrap(), "2"); // shadow
            }
            assert_eq!(require_env("ER_A").unwrap(), "1"); // restored
        }
        assert!(matches!(require_env("ER_A"), Err(EnvError::Missing(_)))); // cleared
    }
}
