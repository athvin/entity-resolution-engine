//! The S5.2 counter vocabulary (port of `src/er/obs/counters.py`): eleven
//! promoted (typed-column) counters, everything else free-form in the JSON
//! payload. `set` routes a name to its promoted column if listed, else the
//! payload — keeping the S5.2 completeness rule true by construction.
//!
//! The payload JSON is built in insertion order manually (declared → promoted →
//! extra), matching Python's `json.dumps(separators=(",",":"), ensure_ascii=False)`
//! for scalar values without enabling serde_json's preserve_order workspace-wide.
//! (Nested-object value key order is a later refinement; counters here are scalars.)

use std::collections::HashMap;

use er_schema::model::PROMOTED_COUNTERS;
use indexmap::IndexMap;
use serde_json::Value;

/// The per-stage free-form names each S4 subsection declares, keyed by stage.
/// Stages S4 gives no list (`init`/`train`/`maintain`/`reset`) return `None` —
/// "S4 declares none" stays distinguishable from "unlisted".
#[must_use]
pub fn declared_counters(stage: &str) -> Option<&'static [&'static str]> {
    match stage {
        "ingest" => Some(&[
            "files",
            "new_count",
            "changed_count",
            "unchanged_count",
            "tombstone_count",
            "resurrected_count",
            "ingest_batch_id",
            "duration_ms",
        ]),
        "standardize" => Some(&[
            "models_run",
            "stg_rows_appended",
            "std_rows_current",
            "blocking_rows",
            "blocking_keys_by_type",
            "tombstones_excluded",
            "duration_ms",
        ]),
        "match" => Some(&[
            "mode",
            "model_version",
            "tf_snapshot_id",
            "candidate_pairs",
            "pairs_scored",
            "pairs_above_auto_merge",
            "pairs_in_gray_band",
            "review_queue_added",
            "review_queue_refreshed",
            "duration_ms",
        ]),
        "reconcile" => Some(&[
            "affected_entities",
            "affected_edges",
            "label_prop_iterations",
            "clusters_out",
            "entities_created",
            "entities_merged",
            "entities_split",
            "entities_retired",
            "members_added",
            "members_removed",
            "edges_cut",
            "review_queue_added",
            "events_emitted",
            "duration_ms",
        ]),
        "assemble" => Some(&[
            "entities_touched",
            "entities_rebuilt",
            "entities_reaped",
            "lineage_rows",
            "tiebreak_deterministic_count",
            "duration_ms",
        ]),
        _ => None,
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CounterError {
    #[error(
        "{0:?} is not a promoted counter; write it to the counters JSON payload instead (S5.1)"
    )]
    UnknownCounter(String),
    #[error("promoted counter {name:?} takes an int, not {got}")]
    NotAnInteger { name: String, got: &'static str },
}

fn value_type(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(n) if n.is_f64() => "float",
        Value::Number(_) => "int",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// What one stage counted: the promoted columns plus a free-form JSON payload.
#[derive(Debug, Default)]
pub struct StageCounters {
    declared: Vec<String>,
    promoted: HashMap<String, Option<i64>>,
    extra: IndexMap<String, Value>,
}

impl StageCounters {
    #[must_use]
    pub fn new<I, S>(declared: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            declared: declared.into_iter().map(Into::into).collect(),
            promoted: HashMap::new(),
            extra: IndexMap::new(),
        }
    }

    /// Write `name` to its typed `run_stages` column; errors if not promoted.
    pub fn promote(&mut self, name: &str, value: Option<i64>) -> Result<(), CounterError> {
        if !PROMOTED_COUNTERS.contains(&name) {
            return Err(CounterError::UnknownCounter(name.to_string()));
        }
        self.promoted.insert(name.to_string(), value);
        Ok(())
    }

    /// Record `name`, routing it to the promoted column if S5.2 lists it (value
    /// must then be an integer or null), else into the free-form payload.
    pub fn set(&mut self, name: &str, value: Value) -> Result<(), CounterError> {
        if PROMOTED_COUNTERS.contains(&name) {
            let as_int = match &value {
                Value::Null => None,
                Value::Number(n) => {
                    if let Some(i) = n.as_i64() {
                        Some(i)
                    } else if let Some(u) = n.as_u64() {
                        Some(u as i64)
                    } else {
                        return Err(CounterError::NotAnInteger {
                            name: name.to_string(),
                            got: "float",
                        });
                    }
                }
                other => {
                    return Err(CounterError::NotAnInteger {
                        name: name.to_string(),
                        got: value_type(other),
                    });
                }
            };
            self.promote(name, as_int)?;
        } else {
            self.extra.insert(name.to_string(), value);
        }
        Ok(())
    }

    /// The value of promoted counter `name`, or `None` if unwritten; errors if
    /// `name` is not promoted.
    pub fn column(&self, name: &str) -> Result<Option<i64>, CounterError> {
        if !PROMOTED_COUNTERS.contains(&name) {
            return Err(CounterError::UnknownCounter(name.to_string()));
        }
        Ok(self.promoted.get(name).copied().flatten())
    }

    /// Every promoted counter in `PROMOTED_COUNTERS` order, `None` for unset.
    #[must_use]
    pub fn columns(&self) -> Vec<Option<i64>> {
        PROMOTED_COUNTERS
            .iter()
            .map(|name| self.promoted.get(*name).copied().flatten())
            .collect()
    }

    /// The `run_stages.counters` object: declared (null) first, then promoted
    /// written, then extra — in that stable order.
    #[must_use]
    pub fn payload(&self) -> IndexMap<String, Value> {
        let mut merged: IndexMap<String, Value> = IndexMap::new();
        for name in &self.declared {
            merged.insert(name.clone(), Value::Null);
        }
        for name in PROMOTED_COUNTERS {
            if let Some(value) = self.promoted.get(name) {
                merged.insert(name.to_string(), value.map_or(Value::Null, Value::from));
            }
        }
        for (name, value) in &self.extra {
            merged.insert(name.clone(), value.clone());
        }
        merged
    }

    /// `payload` as the compact JSON the `counters` column stores (insertion order).
    #[must_use]
    pub fn payload_json(&self) -> String {
        let parts: Vec<String> = self
            .payload()
            .iter()
            .map(|(key, value)| {
                format!(
                    "{}:{}",
                    serde_json::to_string(key).expect("string key"),
                    serde_json::to_string(value).expect("json value")
                )
            })
            .collect();
        format!("{{{}}}", parts.join(","))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn promote_rejects_unlisted_name() {
        let mut c = StageCounters::new(Vec::<String>::new());
        assert!(matches!(
            c.promote("files", Some(1)),
            Err(CounterError::UnknownCounter(_))
        ));
        assert!(c.promote("rows_in", Some(1)).is_ok());
    }

    #[test]
    fn set_routes_and_type_checks() {
        let mut c = StageCounters::new(Vec::<String>::new());
        c.set("candidate_pairs", json!(10)).unwrap(); // promoted -> column
        assert_eq!(c.column("candidate_pairs").unwrap(), Some(10));
        c.set("mode", json!("full")).unwrap(); // free-form -> extra
        assert!(matches!(
            c.set("rows_in", json!("x")),
            Err(CounterError::NotAnInteger { .. })
        ));
        assert!(matches!(
            c.set("rows_in", json!(1.5)),
            Err(CounterError::NotAnInteger { .. })
        ));
    }

    #[test]
    fn columns_in_promoted_order_with_nulls() {
        let mut c = StageCounters::new(Vec::<String>::new());
        c.promote("rows_in", Some(3)).unwrap();
        let cols = c.columns();
        assert_eq!(cols.len(), PROMOTED_COUNTERS.len());
        assert_eq!(cols[0], Some(3)); // rows_in is first
        assert_eq!(cols[1], None); // rows_out unwritten
    }

    #[test]
    fn payload_is_declared_then_promoted_then_extra() {
        // declared includes "files" (free-form) and "duration_ms" (promoted).
        let mut c = StageCounters::new(["files", "duration_ms"]);
        c.set("files", json!(2)).unwrap(); // declared + free-form -> value 2, keeps position
        c.set("duration_ms", json!(15)).unwrap(); // declared + promoted
        assert_eq!(c.payload_json(), r#"{"files":2,"duration_ms":15}"#);

        // A promoted-but-not-declared name appends after declared; extra last.
        let mut d = StageCounters::new(["files"]);
        d.promote("rows_in", Some(7)).unwrap();
        d.set("mode", json!("full")).unwrap();
        assert_eq!(
            d.payload_json(),
            r#"{"files":null,"rows_in":7,"mode":"full"}"#
        );
    }
}
