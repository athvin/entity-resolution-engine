use thiserror::Error;

#[derive(Debug, Error)]
pub enum DedupeError {
    #[error("settings error: {0}")]
    Settings(String),

    #[error("SQL parse error in {context}: {message}")]
    SqlParse { context: String, message: String },

    #[error("database error: {0}")]
    Db(#[from] duckdb::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("I/O error: {0}")]
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
