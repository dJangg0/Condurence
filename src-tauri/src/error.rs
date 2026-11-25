use serde::{Serialize, Serializer};

/// Error surfaced to the frontend. Serialized as a plain string so the UI can
/// show it directly.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("{0}")]
    Message(String),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

impl AppError {
    pub fn msg(text: impl Into<String>) -> Self {
        AppError::Message(text.into())
    }
}

impl Serialize for AppError {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

pub type AppResult<T> = Result<T, AppError>;

/// Early-returns an `AppError::Message` built from a format string.
macro_rules! bail {
    ($($arg:tt)*) => {
        return Err($crate::error::AppError::msg(format!($($arg)*)))
    };
}
pub(crate) use bail;
