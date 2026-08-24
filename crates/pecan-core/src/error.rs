//! Typed errors for the pecan domain layer.

use std::path::PathBuf;

/// Errors produced by pecan-core operations.
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    /// The home directory could not be resolved.
    #[error("home directory not found")]
    HomeMissing,
    /// An I/O operation on a pi-managed file failed.
    #[error("io failed for {}: {source}", path.display())]
    Io {
        /// File involved in the failed operation.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// A SQLite state-store operation failed.
    #[error("database error: {0}")]
    Sql(#[from] rusqlite::Error),
    /// A shared-state mutex was poisoned by a panicking writer.
    #[error("state store lock poisoned")]
    LockPoisoned,
    /// A background task failed before returning a result.
    #[error("background task failed")]
    Join,
    /// A pi-managed JSON/JSONL file could not be parsed where strictness was required.
    #[error("json failed for {}: {source}", path.display())]
    Json {
        /// File involved in the failed parse.
        path: PathBuf,
        /// Underlying JSON error.
        #[source]
        source: serde_json::Error,
    },
}

/// Convenience alias used throughout pecan-core.
pub type Result<T> = std::result::Result<T, CoreError>;
