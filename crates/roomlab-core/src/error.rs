use crate::contracts::ReasonCode;

#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("database error: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    /// A validated rejection with a contract reason code (§13.5).
    #[error("{code:?}: {message}")]
    Rejected { code: ReasonCode, message: String },
    #[error("not found: {0}")]
    NotFound(String),
    #[error("invalid input: {0}")]
    Invalid(String),
    #[error("provider error: {0}")]
    Provider(String),
    #[error("forbidden: {0}")]
    Forbidden(String),
}

pub type Result<T> = std::result::Result<T, CoreError>;

pub fn reject<T>(code: ReasonCode, message: impl Into<String>) -> Result<T> {
    Err(CoreError::Rejected { code, message: message.into() })
}

impl CoreError {
    pub fn reason_code(&self) -> Option<ReasonCode> {
        match self {
            CoreError::Rejected { code, .. } => Some(*code),
            _ => None,
        }
    }
}
