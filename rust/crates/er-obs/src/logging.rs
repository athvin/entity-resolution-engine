//! The S5.2 structured stage log line (port of `src/er/obs/logging.py`): one
//! compact JSON object per stage, a fixed ordered key set (`exit_code` after
//! `status`, the eleven promoted counters embedded), written to stderr by the
//! caller. [`stage_record_line`] formats it; the key set is validated exactly.
//!
//! # This has no callers, and what it is waiting for
//!
//! [`stage_record_line`] is complete and tested and nothing calls it. That is
//! worth stating, because an unwired primitive reads as either an oversight or
//! as dead code, and it is neither.
//!
//! It is waiting on the **eleven promoted counters**, not on a call site. Its
//! key set is validated exactly against `PROMOTED_COUNTERS`, so a caller must
//! supply all of them or get `KeySetMismatch` — and the one obvious caller,
//! `er_cli`'s `observed()`, does not have them. `observed()` brackets a stage
//! and returns its result generically; the counters come from each stage's own
//! output, so producing them means each of the eleven stages reporting its own,
//! which is the substance of that work rather than a detail of it.
//!
//! Calling this with the counters defaulted to null or zero would be worse than
//! not calling it. `run_stages` would fill, the `cli_e2e` tripwire asserting
//! that table stays empty would flip, and every dashboard over it would read
//! zeros as measurements. The same reason `docs/telemetry-reference.md` is
//! generated rather than hand-written: a thing that looks like coverage is
//! worse than a visible gap.
//!
//! So this stays unwired deliberately, and lands with the `run_stages` writer
//! (batch-orchestration-design §4.3), which is also where
//! [`crate::counters`] gets its first caller.
//!
//! One correction while here: `er-cli/tests/cli_e2e.rs` says `run_stages`'s
//! "writer is the Phase 2 pod verb". It is not — `exec_task.rs` does not
//! reference the entity, and a search of the workspace finds **zero** callers
//! of `er_entities::run_stage`'s generated `start` / `finish` anywhere. The
//! assertion is right about the state and wrong about the reason, which is the
//! kind of comment that survives precisely because the test passes — a trap
//! that same file warns about two lines earlier.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;

use er_schema::model::PROMOTED_COUNTERS;
use serde_json::Value;

/// The S5.2 example line's keys, in order, with `exit_code` after `status` and
/// the eleven promoted counters embedded (from the registry, not re-typed).
pub static STAGE_RECORD_KEYS: LazyLock<Vec<&'static str>> = LazyLock::new(|| {
    let mut keys = vec![
        "run_id",
        "stage",
        "seq",
        "status",
        "exit_code",
        "started_at",
        "ended_at",
        "snapshot_start",
        "snapshot_end",
        "config_hash",
        "model_version",
        "tf_snapshot_id",
    ];
    keys.extend_from_slice(&PROMOTED_COUNTERS);
    keys.extend_from_slice(&["error_class", "error_detail"]);
    keys
});

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("stage record key set is not S5.2's: missing={missing:?}, extra={extra:?}")]
pub struct KeySetMismatch {
    pub missing: Vec<String>,
    pub extra: Vec<String>,
}

/// Format `record` as exactly one compact JSON line in `STAGE_RECORD_KEYS` order.
/// The key set MUST equal `STAGE_RECORD_KEYS` (missing/extra are named). Returns
/// the line without a trailing newline; the caller writes it to stderr.
pub fn stage_record_line(record: &BTreeMap<String, Value>) -> Result<String, KeySetMismatch> {
    let keys = &*STAGE_RECORD_KEYS;
    let missing: Vec<String> = keys
        .iter()
        .filter(|k| !record.contains_key(**k))
        .map(|k| (*k).to_string())
        .collect();
    let keyset: BTreeSet<&str> = keys.iter().copied().collect();
    let extra: Vec<String> = record
        .keys()
        .filter(|k| !keyset.contains(k.as_str()))
        .cloned()
        .collect();
    if !missing.is_empty() || !extra.is_empty() {
        return Err(KeySetMismatch { missing, extra });
    }
    let parts: Vec<String> = keys
        .iter()
        .map(|k| {
            format!(
                "{}:{}",
                serde_json::to_string(k).expect("string key"),
                serde_json::to_string(&record[*k]).expect("json value")
            )
        })
        .collect();
    Ok(format!("{{{}}}", parts.join(",")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn full_record() -> BTreeMap<String, Value> {
        STAGE_RECORD_KEYS
            .iter()
            .map(|k| ((*k).to_string(), Value::Null))
            .collect()
    }

    #[test]
    fn key_set_has_25_keys_in_contract_order() {
        let keys = &*STAGE_RECORD_KEYS;
        assert_eq!(keys.len(), 12 + PROMOTED_COUNTERS.len() + 2);
        assert_eq!(keys.len(), 25);
        assert_eq!(keys[0], "run_id");
        assert_eq!(keys[4], "exit_code"); // after status
        assert_eq!(keys[12], "rows_in"); // promoted block starts here
        assert_eq!(*keys.last().unwrap(), "error_detail");
    }

    #[test]
    fn line_is_ordered_and_complete() {
        let mut record = full_record();
        record.insert("run_id".to_string(), json!("R1"));
        record.insert("stage".to_string(), json!("ingest"));
        record.insert("exit_code".to_string(), json!(0));
        let line = stage_record_line(&record).unwrap();
        assert!(line.starts_with(
            r#"{"run_id":"R1","stage":"ingest","seq":null,"status":null,"exit_code":0,"#
        ));
        assert!(line.ends_with(r#""error_detail":null}"#));
        // round-trips to a 25-key object
        let parsed: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(parsed.as_object().unwrap().len(), 25);
    }

    #[test]
    fn missing_and_extra_keys_are_rejected() {
        let mut short = full_record();
        short.remove("snapshot_end");
        let err = stage_record_line(&short).unwrap_err();
        assert_eq!(err.missing, vec!["snapshot_end".to_string()]);

        let mut wide = full_record();
        wide.insert("surprise".to_string(), Value::Null);
        let err = stage_record_line(&wide).unwrap_err();
        assert_eq!(err.extra, vec!["surprise".to_string()]);
    }
}
