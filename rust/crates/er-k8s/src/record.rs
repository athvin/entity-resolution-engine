//! The ≤ 4 KiB terminal record a pod writes to `/dev/termination-log` on every
//! deliberate exit path AND posts to `finish` (§6.3 step 7). The kubelet caps
//! the message at 4096 bytes, so the detail is truncated until the JSON always
//! parses — a record that overflows into unparseability is a record the watch
//! cannot read.

use serde::{Deserialize, Serialize};

/// The kubelet's cap on `terminationMessagePath` content.
pub const TERMINATION_MESSAGE_CAP: usize = 4096;

/// The record's `state` — the §6.3 vocabulary, narrower than the row's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalState {
    Succeeded,
    Skipped,
    Failed,
    /// SIGTERM, rolled back, NO verdict: the watch decides from the platform's
    /// evidence once the pod is terminal (§5.6).
    Interrupted,
}

/// The terminal record itself.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TerminalRecord {
    /// Record-format version; readers refuse an unknown major.
    pub v: u32,
    /// The attempt (task_instances.id) this record describes.
    pub attempt: String,
    pub state: TerminalState,
    pub exit: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_class: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub termination_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_start: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_end: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub counters: Option<serde_json::Value>,
}

impl TerminalRecord {
    /// Serialize under the kubelet's cap: the detail is cut (character-safe)
    /// until the whole document fits, because a truncated JSON byte stream is
    /// unreadable where a truncated detail string is merely shorter.
    #[must_use]
    pub fn to_capped_json(&self) -> String {
        let mut record = self.clone();
        loop {
            let json = serde_json::to_string(&record).unwrap_or_else(|_| {
                // Unrepresentable output (a non-string map key) — drop it and
                // keep the verdict.
                record.output = None;
                serde_json::to_string(&record).expect("a record without output serializes")
            });
            if json.len() <= TERMINATION_MESSAGE_CAP {
                return json;
            }
            match record.detail.as_mut() {
                Some(detail) if !detail.is_empty() => {
                    let overflow = json.len() - TERMINATION_MESSAGE_CAP;
                    // Cut the overflow plus slack (escapes shrink too), on a
                    // char boundary.
                    let mut cut = detail.len().saturating_sub(overflow.saturating_add(64));
                    while cut > 0 && !detail.is_char_boundary(cut) {
                        cut -= 1;
                    }
                    detail.truncate(cut);
                }
                _ if record.output.is_some() => record.output = None,
                _ if record.counters.is_some() => record.counters = None,
                _ => {
                    // Nothing left to shed; serialize what there is (a pathological
                    // attempt id — still bounded input from our own planner).
                    return json;
                }
            }
        }
    }

    /// Parse a termination message. `None` when the message is not this
    /// record's shape — a crash's stray output is evidence, not a verdict.
    #[must_use]
    pub fn parse(message: &str) -> Option<Self> {
        let record: TerminalRecord = serde_json::from_str(message.trim()).ok()?;
        if record.v != 1 {
            return None;
        }
        Some(record)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(detail: &str) -> TerminalRecord {
        TerminalRecord {
            v: 1,
            attempt: "a".repeat(64),
            state: TerminalState::Failed,
            exit: 1,
            error_class: Some("data".into()),
            termination_reason: None,
            detail: Some(detail.to_string()),
            output: None,
            snapshot_start: None,
            snapshot_end: Some(12),
            counters: None,
        }
    }

    #[test]
    fn a_record_round_trips() {
        let r = record("boom");
        let parsed = TerminalRecord::parse(&r.to_capped_json()).expect("parses");
        assert_eq!(parsed.detail.as_deref(), Some("boom"));
        assert_eq!(parsed.state, TerminalState::Failed);
        assert_eq!(parsed.snapshot_end, Some(12));
    }

    #[test]
    fn an_oversized_detail_is_cut_until_the_json_fits() {
        let r = record(&"x".repeat(10_000));
        let json = r.to_capped_json();
        assert!(
            json.len() <= TERMINATION_MESSAGE_CAP,
            "{} bytes",
            json.len()
        );
        let parsed = TerminalRecord::parse(&json).expect("still parses");
        assert!(parsed.detail.unwrap().len() < 10_000);
    }

    #[test]
    fn non_record_messages_and_foreign_versions_are_not_verdicts() {
        assert!(TerminalRecord::parse("panic: index out of bounds").is_none());
        assert!(
            TerminalRecord::parse("{\"v\":2,\"attempt\":\"x\",\"state\":\"failed\",\"exit\":1}")
                .is_none()
        );
    }
}
