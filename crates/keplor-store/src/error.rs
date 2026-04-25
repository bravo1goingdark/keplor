//! Storage-subsystem errors.

/// Errors produced by the local store.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// SQLite operation failed (only present when the
    /// `migrate-from-sqlite` feature is enabled).
    #[cfg(feature = "migrate-from-sqlite")]
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

    /// Internal error (e.g. `spawn_blocking` panic).
    #[error("internal: {0}")]
    Internal(String),

    /// Filesystem / stdio error.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    /// Backend-specific error not otherwise classified.
    #[error("{0}")]
    Other(String),

    /// The batch writer channel is full — back-pressure signal.
    #[error("batch writer channel full")]
    ChannelFull,

    /// The batch writer channel is closed — writer shut down.
    #[error("batch writer channel closed")]
    ChannelClosed,

    /// A `std::sync::Mutex` was poisoned by a panicking thread.
    #[error("lock poisoned: {0}")]
    LockPoisoned(String),

    /// S3/R2 archive operation failed.
    #[cfg(feature = "s3")]
    #[error("archive (s3): {0}")]
    ArchiveS3(String),
}
