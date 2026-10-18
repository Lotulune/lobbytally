use thiserror::Error;

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("postgresql error: {0}")]
    Database(#[from] crate::pg::Error),

    #[error("migration error: {message}")]
    Migration { message: String },

    #[error("not found: {entity}")]
    NotFound { entity: String },

    #[error("conflict: {message}")]
    Conflict { message: String },

    #[error("validation error: {message}")]
    Validation { message: String },

    #[error("lease error: {message}")]
    Lease { message: String },

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}

impl StorageError {
    pub fn migration(message: impl Into<String>) -> Self {
        Self::Migration {
            message: message.into(),
        }
    }

    pub fn not_found(entity: impl Into<String>) -> Self {
        Self::NotFound {
            entity: entity.into(),
        }
    }

    pub fn validation(message: impl Into<String>) -> Self {
        Self::Validation {
            message: message.into(),
        }
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self::Conflict {
            message: message.into(),
        }
    }

    pub fn lease(message: impl Into<String>) -> Self {
        Self::Lease {
            message: message.into(),
        }
    }
}

pub type StorageResult<T> = Result<T, StorageError>;
