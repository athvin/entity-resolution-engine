//! The config-drift guard (drift-guard half of `src/er/versions.py`): the run
//! fingerprint, the S4.0 mode-precondition decision, and the S5.1 rebuild-reason.
//! Pure — reads nothing, opens nothing. (The importlib pin table + the duckdb
//! `runs` readers are a later wave.)

use std::fmt;

pub const STATUS_SUCCEEDED: &str = "succeeded";
pub const MODE_INCREMENTAL: &str = "incremental";
pub const MODE_FULL: &str = "full";
pub const MODE_CORRECTION_PASS: &str = "correction_pass";

pub const REBUILD_STD_VERSION_BUMP: &str = "std_version_bump";
pub const REBUILD_SURVIVORSHIP_VERSION_BUMP: &str = "survivorship_version_bump";
pub const REBUILD_CORRECTION_PASS: &str = "correction_pass";
pub const REBUILD_OPERATOR: &str = "operator";
pub const REBUILD_LEXICON_CHANGE: &str = "lexicon_change";

/// The five `runs` columns the guard compares, in S5 DDL order.
pub const FINGERPRINT_FIELDS: [&str; 5] = [
    "config_hash",
    "model_version",
    "std_version",
    "survivorship_version",
    "lexicon_hash",
];

/// How a NULL renders in a refusal.
pub const UNSET_VALUE: &str = "(unset)";

const REFUSAL_MESSAGE: &str = "incremental run refused: the run fingerprint differs from the last successful run: {drift}. Re-run with --mode full, or with --allow-escalate to promote this run to --mode full.";
const ESCALATION_MESSAGE: &str = "--allow-escalate: the run fingerprint differs from the last successful run: {drift}. Promoting this run to --mode full.";

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum VersionsError {
    #[error("not a fingerprint field: {0:?}")]
    UnknownField(String),
    #[error("unknown run-all mode: {0:?}")]
    UnknownMode(String),
}

/// The five `runs` columns that decide whether an incremental run may proceed.
/// `model_version` and `lexicon_hash` are nullable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunFingerprint {
    pub config_hash: String,
    pub model_version: Option<String>,
    pub std_version: String,
    pub survivorship_version: String,
    pub lexicon_hash: Option<String>,
}

impl RunFingerprint {
    /// The value of one [`FINGERPRINT_FIELDS`] entry; errors on an unknown field
    /// (a typo would otherwise compare two `None`s as equal — a guard that stopped
    /// guarding).
    pub fn value(&self, field: &str) -> Result<Option<&str>, VersionsError> {
        match field {
            "config_hash" => Ok(Some(self.config_hash.as_str())),
            "model_version" => Ok(self.model_version.as_deref()),
            "std_version" => Ok(Some(self.std_version.as_str())),
            "survivorship_version" => Ok(Some(self.survivorship_version.as_str())),
            "lexicon_hash" => Ok(self.lexicon_hash.as_deref()),
            other => Err(VersionsError::UnknownField(other.to_string())),
        }
    }

    fn field_value(&self, field: &str) -> Option<&str> {
        self.value(field).expect("FINGERPRINT_FIELDS entry")
    }
}

fn rendered(value: Option<&str>) -> &str {
    value.unwrap_or(UNSET_VALUE)
}

/// One field of the fingerprint that moved, with both of its values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldDrift {
    pub field: String,
    pub prior: Option<String>,
    pub current: Option<String>,
}

impl fmt::Display for FieldDrift {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} last={} now={}",
            self.field,
            rendered(self.prior.as_deref()),
            rendered(self.current.as_deref())
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModeOutcome {
    Proceed,
    Escalate,
    Refuse,
}

impl ModeOutcome {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            ModeOutcome::Proceed => "proceed",
            ModeOutcome::Escalate => "escalate",
            ModeOutcome::Refuse => "refuse",
        }
    }
}

/// The guard's verdict: the mode to run under, why, the drift, and the
/// operator-facing message (empty when nothing drifted).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModeDecision {
    pub outcome: ModeOutcome,
    pub mode: String,
    pub drift: Vec<FieldDrift>,
    pub message: String,
}

impl ModeDecision {
    #[must_use]
    pub fn refused(&self) -> bool {
        self.outcome == ModeOutcome::Refuse
    }
    #[must_use]
    pub fn escalated(&self) -> bool {
        self.outcome == ModeOutcome::Escalate
    }
}

/// Decide whether `mode` may run against `prior` (S4.0). `--mode full` is never
/// guarded; no prior run is no drift; any single fingerprint difference is drift;
/// `--allow-escalate` promotes to `full` rather than refusing.
pub fn check_mode_preconditions(
    prior: Option<&RunFingerprint>,
    current: &RunFingerprint,
    mode: &str,
    allow_escalate: bool,
) -> Result<ModeDecision, VersionsError> {
    if mode != MODE_INCREMENTAL && mode != MODE_FULL {
        return Err(VersionsError::UnknownMode(mode.to_string()));
    }
    let proceed = || ModeDecision {
        outcome: ModeOutcome::Proceed,
        mode: mode.to_string(),
        drift: Vec::new(),
        message: String::new(),
    };
    let Some(prior) = prior else {
        return Ok(proceed());
    };
    if mode != MODE_INCREMENTAL {
        return Ok(proceed());
    }

    let drift: Vec<FieldDrift> = FINGERPRINT_FIELDS
        .iter()
        .filter(|&&name| prior.field_value(name) != current.field_value(name))
        .map(|&name| FieldDrift {
            field: name.to_string(),
            prior: prior.field_value(name).map(str::to_string),
            current: current.field_value(name).map(str::to_string),
        })
        .collect();
    if drift.is_empty() {
        return Ok(proceed());
    }

    let rendered_drift = drift
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("; ");
    if allow_escalate {
        Ok(ModeDecision {
            outcome: ModeOutcome::Escalate,
            mode: MODE_FULL.to_string(),
            drift,
            message: ESCALATION_MESSAGE.replace("{drift}", &rendered_drift),
        })
    } else {
        Ok(ModeDecision {
            outcome: ModeOutcome::Refuse,
            mode: mode.to_string(),
            drift,
            message: REFUSAL_MESSAGE.replace("{drift}", &rendered_drift),
        })
    }
}

/// The value `runs.rebuild_reason` takes for this invocation (S5.1, S4.0). `None`
/// is the ordinary case (the run is inside T-INC-2's accounting). Order of causes
/// is load-bearing: correction_pass, operator, (no prior), std bump, survivorship
/// bump, lexicon change.
#[must_use]
pub fn rebuild_reason_for(
    prior: Option<&RunFingerprint>,
    current: &RunFingerprint,
    mode: Option<&str>,
    operator: bool,
) -> Option<String> {
    if mode == Some(MODE_CORRECTION_PASS) {
        return Some(REBUILD_CORRECTION_PASS.to_string());
    }
    if operator {
        return Some(REBUILD_OPERATOR.to_string());
    }
    let prior = prior?;
    if prior.std_version != current.std_version {
        return Some(REBUILD_STD_VERSION_BUMP.to_string());
    }
    if prior.survivorship_version != current.survivorship_version {
        return Some(REBUILD_SURVIVORSHIP_VERSION_BUMP.to_string());
    }
    if prior.lexicon_hash != current.lexicon_hash {
        return Some(REBUILD_LEXICON_CHANGE.to_string());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fp(config: &str) -> RunFingerprint {
        RunFingerprint {
            config_hash: config.to_string(),
            model_version: Some("m1".to_string()),
            std_version: "s1".to_string(),
            survivorship_version: "v1".to_string(),
            lexicon_hash: Some("l1".to_string()),
        }
    }

    #[test]
    fn full_mode_and_no_prior_proceed() {
        let cur = fp("c1");
        assert_eq!(
            check_mode_preconditions(None, &cur, "incremental", false)
                .unwrap()
                .outcome,
            ModeOutcome::Proceed
        );
        // full is never guarded even with drift
        let d = check_mode_preconditions(Some(&fp("c0")), &cur, "full", false).unwrap();
        assert_eq!(d.outcome, ModeOutcome::Proceed);
        assert_eq!(d.mode, "full");
        assert!(matches!(
            check_mode_preconditions(None, &cur, "bogus", false),
            Err(VersionsError::UnknownMode(_))
        ));
    }

    #[test]
    fn incremental_drift_refuses_then_escalates() {
        let prior = fp("c0");
        let current = fp("c1"); // config_hash drifted
        let refuse =
            check_mode_preconditions(Some(&prior), &current, "incremental", false).unwrap();
        assert!(refuse.refused());
        assert_eq!(refuse.mode, "incremental");
        assert!(refuse.message.contains("config_hash last=c0 now=c1"));
        assert!(refuse.message.starts_with("incremental run refused:"));

        let escalate =
            check_mode_preconditions(Some(&prior), &current, "incremental", true).unwrap();
        assert!(escalate.escalated());
        assert_eq!(escalate.mode, "full");
        assert!(escalate.message.starts_with("--allow-escalate:"));
        assert!(escalate.message.contains("config_hash last=c0 now=c1"));
    }

    #[test]
    fn no_drift_proceeds_and_null_renders_unset() {
        let prior = fp("c1");
        assert_eq!(
            check_mode_preconditions(Some(&prior), &fp("c1"), "incremental", false)
                .unwrap()
                .outcome,
            ModeOutcome::Proceed
        );
        // a NULL model_version drift renders (unset)
        let mut current = fp("c1");
        current.model_version = None;
        let d = check_mode_preconditions(Some(&prior), &current, "incremental", false).unwrap();
        assert!(d.message.contains("model_version last=m1 now=(unset)"));
    }

    #[test]
    fn rebuild_reason_precedence() {
        let prior = fp("c0");
        assert_eq!(
            rebuild_reason_for(Some(&prior), &fp("c0"), Some("correction_pass"), false).as_deref(),
            Some("correction_pass")
        );
        assert_eq!(
            rebuild_reason_for(Some(&prior), &fp("c0"), None, true).as_deref(),
            Some("operator")
        );
        assert_eq!(rebuild_reason_for(None, &fp("c0"), None, false), None);
        let mut std_bump = fp("c0");
        std_bump.std_version = "s2".to_string();
        std_bump.survivorship_version = "v2".to_string(); // both moved -> std wins
        assert_eq!(
            rebuild_reason_for(Some(&prior), &std_bump, None, false).as_deref(),
            Some("std_version_bump")
        );
        let mut surv = fp("c0");
        surv.survivorship_version = "v2".to_string();
        assert_eq!(
            rebuild_reason_for(Some(&prior), &surv, None, false).as_deref(),
            Some("survivorship_version_bump")
        );
        let mut lex = fp("c0");
        lex.lexicon_hash = Some("l2".to_string());
        assert_eq!(
            rebuild_reason_for(Some(&prior), &lex, None, false).as_deref(),
            Some("lexicon_change")
        );
        assert_eq!(
            rebuild_reason_for(Some(&prior), &fp("c0"), None, false),
            None
        );
    }

    #[test]
    fn value_rejects_unknown_field() {
        assert!(matches!(
            fp("c0").value("nope"),
            Err(VersionsError::UnknownField(_))
        ));
        assert_eq!(fp("c0").value("config_hash").unwrap(), Some("c0"));
    }
}
