//! The S4.7 failure taxonomy and the S4.0 exit-code contract (Rust port of
//! `src/er/errors.py`).
//!
//! `ErrorClass::exit_code` / `::retryable` are the single transcription of
//! S4.7's table (derived once, never written twice). Exit `10` sits outside the
//! taxonomy: it is a successful no-op (`nothing_to_do`) with no error class.
//! Python exceptions become an [`ErError`] with per-kind constructors.

use std::fmt;

/// The S4.0 exit statuses, uniform across every `er` command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitCode {
    Success,
    StageFailure,
    Config,
    Precondition,
    NothingToDo,
}

impl ExitCode {
    #[must_use]
    pub fn code(self) -> i32 {
        match self {
            ExitCode::Success => 0,
            ExitCode::StageFailure => 1,
            ExitCode::Config => 2,
            ExitCode::Precondition => 3,
            ExitCode::NothingToDo => 10,
        }
    }
}

/// The values `run_stages.error_class` may take (S4.7). The spelling returned by
/// [`ErrorClass::as_str`] IS the stored `VARCHAR` / S5.2 log value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorClass {
    TransientIo,
    LockConflict,
    Precondition,
    Config,
    Contradiction,
    NonConvergence,
    Data,
}

/// The class an unclassified failure is recorded under (S4.7 gives `data` exit 1).
pub const UNCLASSIFIED: ErrorClass = ErrorClass::Data;

impl ErrorClass {
    /// Every class, for exhaustiveness tests.
    pub const ALL: [ErrorClass; 7] = [
        ErrorClass::TransientIo,
        ErrorClass::LockConflict,
        ErrorClass::Precondition,
        ErrorClass::Config,
        ErrorClass::Contradiction,
        ErrorClass::NonConvergence,
        ErrorClass::Data,
    ];

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorClass::TransientIo => "transient_io",
            ErrorClass::LockConflict => "lock_conflict",
            ErrorClass::Precondition => "precondition",
            ErrorClass::Config => "config",
            ErrorClass::Contradiction => "contradiction",
            ErrorClass::NonConvergence => "non_convergence",
            ErrorClass::Data => "data",
        }
    }

    /// S4.7's "Exit" column — the single transcription.
    #[must_use]
    pub fn exit_code(self) -> ExitCode {
        match self {
            ErrorClass::LockConflict | ErrorClass::Precondition => ExitCode::Precondition,
            ErrorClass::Config => ExitCode::Config,
            ErrorClass::TransientIo
            | ErrorClass::Contradiction
            | ErrorClass::NonConvergence
            | ErrorClass::Data => ExitCode::StageFailure,
        }
    }

    /// S4.7's "Retryable" column — true for exactly the two classes whose cause
    /// is outside the run. Advisory to an operator; nothing acts on it.
    #[must_use]
    pub fn retryable(self) -> bool {
        matches!(self, ErrorClass::TransientIo | ErrorClass::LockConflict)
    }
}

impl fmt::Display for ErrorClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// An error the CLI translates into an S4.0 exit status.
///
/// `error_class` is `None` only for `nothing_to_do` (exit 10), which S4.7 does
/// not classify because it is not a failure.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{message}")]
pub struct ErError {
    pub message: String,
    pub error_class: Option<ErrorClass>,
    pub detail: String,
    unclassified_exit: ExitCode,
}

impl ErError {
    fn new(
        message: impl Into<String>,
        error_class: Option<ErrorClass>,
        unclassified_exit: ExitCode,
    ) -> Self {
        let message = message.into();
        let detail = message.clone();
        Self {
            message,
            error_class,
            detail,
            unclassified_exit,
        }
    }

    /// An operation was attempted and failed (exit 1); default class `data`.
    pub fn stage_failure(message: impl Into<String>) -> Self {
        Self::new(message, Some(ErrorClass::Data), ExitCode::StageFailure)
    }
    /// A bounded fixpoint did not settle (exit 1, class `non_convergence`).
    pub fn non_convergence(message: impl Into<String>) -> Self {
        Self::new(
            message,
            Some(ErrorClass::NonConvergence),
            ExitCode::StageFailure,
        )
    }
    /// A stage precondition does not hold (exit 3, class `precondition`).
    pub fn precondition(message: impl Into<String>) -> Self {
        Self::new(
            message,
            Some(ErrorClass::Precondition),
            ExitCode::StageFailure,
        )
    }
    /// The configuration or invocation is invalid (exit 2, class `config`).
    pub fn config(message: impl Into<String>) -> Self {
        Self::new(message, Some(ErrorClass::Config), ExitCode::StageFailure)
    }
    /// Nothing to do (exit 10): a successful no-op, no class.
    pub fn nothing_to_do(message: impl Into<String>) -> Self {
        Self::new(message, None, ExitCode::NothingToDo)
    }

    /// Override the recorded class (e.g. a `StageFailure` with `transient_io`).
    #[must_use]
    pub fn with_class(mut self, error_class: ErrorClass) -> Self {
        self.error_class = Some(error_class);
        self
    }
    /// Override `run_stages.error_detail` (defaults to `message`).
    #[must_use]
    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = detail.into();
        self
    }

    /// The S4.0 exit status, derived from `error_class`.
    #[must_use]
    pub fn code(&self) -> i32 {
        match self.error_class {
            Some(class) => class.exit_code().code(),
            None => self.unclassified_exit.code(),
        }
    }

    /// S4.7's advisory retry verdict. Nothing in the CLI acts on it.
    #[must_use]
    pub fn retryable(&self) -> bool {
        self.error_class.is_some_and(ErrorClass::retryable)
    }

    /// The S4.7 row this is recorded under: its class, or `UNCLASSIFIED`.
    #[must_use]
    pub fn classify(&self) -> ErrorClass {
        self.error_class.unwrap_or(UNCLASSIFIED)
    }
}

/// The exit status for a foreign error exposing an integer `code` (the
/// convention for modules that predate the taxonomy, e.g. config loader → 2).
/// Anything else, or an unknown code, is exit 1.
#[must_use]
pub fn exit_code_for(code: Option<i32>) -> i32 {
    const KNOWN: [i32; 5] = [0, 1, 2, 3, 10];
    match code {
        Some(c) if KNOWN.contains(&c) => c,
        _ => ExitCode::StageFailure.code(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // S4.7 table, row for row (exit column + retryable column).
    #[test]
    fn s47_table_transcription() {
        let rows = [
            (ErrorClass::TransientIo, 1, true),
            (ErrorClass::LockConflict, 3, true),
            (ErrorClass::Precondition, 3, false),
            (ErrorClass::Config, 2, false),
            (ErrorClass::Contradiction, 1, false),
            (ErrorClass::NonConvergence, 1, false),
            (ErrorClass::Data, 1, false),
        ];
        for (class, exit, retryable) in rows {
            assert_eq!(class.exit_code().code(), exit, "{} exit", class.as_str());
            assert_eq!(class.retryable(), retryable, "{} retryable", class.as_str());
        }
        // retryable is true for exactly the two external-cause classes.
        assert_eq!(ErrorClass::ALL.iter().filter(|c| c.retryable()).count(), 2);
    }

    #[test]
    fn stored_spellings() {
        assert_eq!(ErrorClass::TransientIo.as_str(), "transient_io");
        assert_eq!(ErrorClass::NonConvergence.as_str(), "non_convergence");
        assert_eq!(ErrorClass::Data.to_string(), "data");
    }

    // Each exception kind's code + class, matching the Python subclasses.
    #[test]
    fn exception_kinds() {
        assert_eq!(ErError::stage_failure("x").code(), 1);
        assert_eq!(ErError::stage_failure("x").classify(), ErrorClass::Data);
        assert!(!ErError::stage_failure("x").retryable());

        // StageFailure with a precise class.
        let io = ErError::stage_failure("s3 5xx").with_class(ErrorClass::TransientIo);
        assert_eq!(io.code(), 1);
        assert!(io.retryable());

        assert_eq!(ErError::non_convergence("loop").code(), 1);
        assert_eq!(
            ErError::non_convergence("loop").classify(),
            ErrorClass::NonConvergence
        );

        assert_eq!(ErError::precondition("drift").code(), 3);
        let lock = ErError::precondition("held").with_class(ErrorClass::LockConflict);
        assert_eq!(lock.code(), 3);
        assert!(lock.retryable());

        assert_eq!(ErError::config("bad yaml").code(), 2);
        assert_eq!(ErError::config("bad yaml").classify(), ErrorClass::Config);

        let ntd = ErError::nothing_to_do("empty");
        assert_eq!(ntd.code(), 10);
        assert_eq!(ntd.error_class, None);
        assert!(!ntd.retryable());
        assert_eq!(ntd.classify(), UNCLASSIFIED); // Data, by the unwrap_or default
    }

    #[test]
    fn detail_defaults_to_message_and_overrides() {
        assert_eq!(ErError::stage_failure("m").detail, "m");
        assert_eq!(
            ErError::stage_failure("m")
                .with_detail("long diagnosis")
                .detail,
            "long diagnosis"
        );
    }

    #[test]
    fn exit_code_for_honours_known_codes_only() {
        assert_eq!(exit_code_for(Some(2)), 2); // config loader convention
        assert_eq!(exit_code_for(Some(10)), 10);
        assert_eq!(exit_code_for(Some(7)), 1); // unknown -> 1
        assert_eq!(exit_code_for(None), 1);
    }
}
