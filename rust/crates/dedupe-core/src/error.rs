use thiserror::Error;

#[derive(Debug, Error)]
pub enum DedupeError {
    #[error("settings error: {0}")]
    Settings(String),

    #[error("SQL parse error in {context}: {message}")]
    SqlParse { context: String, message: String },

    // These three carry their cause as a `#[from]` source, so the source is
    // NOT interpolated into the message: a reporter that walks `.source()`
    // would otherwise print it twice.
    #[error("database error")]
    Db(#[from] duckdb::Error),

    #[error("JSON error")]
    Json(#[from] serde_json::Error),

    #[error("I/O error")]
    Io(#[from] std::io::Error),

    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, DedupeError>;

impl DedupeError {
    pub fn settings(msg: impl Into<String>) -> Self {
        DedupeError::Settings(msg.into())
    }
}
