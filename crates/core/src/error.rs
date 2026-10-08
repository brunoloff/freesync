use serde::{Deserialize, Serialize};
use std::fmt;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    Io,
    Permission,
    Authentication,
    NotFound,
    Cancelled,
    RateLimited,
    Transient,
    AmbiguousOutcome,
    Conflict,
    UnsafePath,
    IncompleteScan,
    LockBusy,
    Unsupported,
    Database,
    InvalidConfig,
    Internal,
}

/// Only authored, credential-free messages cross the application boundary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Error {
    pub code: ErrorCode,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after_secs: Option<u64>,
}

impl Error {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            retry_after_secs: None,
        }
    }
    pub fn retryable(&self) -> bool {
        matches!(
            self.code,
            ErrorCode::Transient | ErrorCode::RateLimited | ErrorCode::AmbiguousOutcome
        )
    }
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for Error {}
impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        let (code, message) = match e.kind() {
            std::io::ErrorKind::PermissionDenied => (
                ErrorCode::Permission,
                "A filesystem operation was denied. Check folder permissions.",
            ),
            std::io::ErrorKind::NotFound => (
                ErrorCode::NotFound,
                "A required filesystem item is unavailable.",
            ),
            _ => (
                ErrorCode::Io,
                "A filesystem operation failed. Check disk availability and free space.",
            ),
        };
        Self::new(code, message)
    }
}
impl From<rusqlite::Error> for Error {
    fn from(_: rusqlite::Error) -> Self {
        Self::new(
            ErrorCode::Database,
            "Application state could not be read or saved.",
        )
    }
}
impl From<serde_json::Error> for Error {
    fn from(_: serde_json::Error) -> Self {
        Self::new(
            ErrorCode::InvalidConfig,
            "Application data has an invalid format.",
        )
    }
}
