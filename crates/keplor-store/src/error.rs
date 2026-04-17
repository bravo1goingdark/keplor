//! Storage-subsystem errors.

/// Errors produced by the local store.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// SQLite operation failed.
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),

    /// Schema migration failed.
    #[error("migration v{version} failed: {reason}")]
    Migration {
        /// Migration version that failed.
        version: u32,
        /// Human-readable reason.
        reason: String,
    },

    /// Zstd compression or decompression failed.
    #[error("zstd: {0}")]
    Compression(String),

    /// Component extraction from the request/response body failed.
    #[error("component extraction failed: {0}")]
    ComponentExtract(String),

    /// Blob integrity check failed (sha256 mismatch on read).
    #[error("blob integrity check failed: expected {expected}, got {actual}")]
    IntegrityCheck {
        /// Expected hex sha256.
        expected: String,
        /// Actual hex sha256.
        actual: String,
    },
}
