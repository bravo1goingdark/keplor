//! Pluggable blob storage backends.
//!
//! The [`BlobStore`] trait abstracts **where compressed payload bytes
//! live**.  Metadata (refcount, sizes, compression tag, dict ID) always
//! stays in the SQLite `payload_blobs` table — only the heavy `data`
//! column moves when you switch backends.
//!
//! Two implementations ship with keplor-store:
//!
//! - [`EmbeddedBlobStore`] — blobs live in the SQLite `payload_blobs.data`
//!   column (zero-dep default).
//! - [`S3BlobStore`] — blobs live in any S3-compatible object store
//!   (Cloudflare R2, MinIO, AWS S3).  Requires the `s3` feature flag.
//!   The `payload_blobs.data` column is left `NULL`; actual bytes are
//!   keyed by SHA-256 hex in the bucket.

pub mod embedded;
#[cfg(feature = "s3")]
pub mod s3;

#[cfg(feature = "s3")]
pub use self::s3::S3BlobStore;
pub use embedded::EmbeddedBlobStore;

use bytes::Bytes;

use crate::error::StoreError;

/// Metadata accompanying a blob on write.
///
/// Passed to [`BlobStore::put`] so the backend can store the compressed
/// bytes alongside any metadata it needs.  For `EmbeddedBlobStore` this
/// is a no-op (metadata goes in the same SQLite row).  For S3 backends
/// the metadata is stored as object tags or ignored.
#[derive(Debug, Clone)]
pub struct BlobMeta<'a> {
    /// Component type (`"system_prompt"`, `"messages"`, `"response"`, `"raw"`).
    pub component_type: &'a str,
    /// Provider key (`"openai"`, `"anthropic"`, ...).
    pub provider: &'a str,
}

/// Pluggable storage backend for compressed payload blobs.
///
/// Handles **only** the compressed byte data.  All metadata (refcount,
/// sizes, compression method, dict ID) is tracked in the SQLite
/// `payload_blobs` table by [`crate::store::Store`], regardless of
/// which `BlobStore` backend is active.
///
/// Implementations must be `Send + Sync` for use behind `Arc` in the
/// async server.  All methods are synchronous — the server wraps calls
/// in `spawn_blocking` when needed.
pub trait BlobStore: Send + Sync + std::fmt::Debug {
    /// Store compressed bytes keyed by their SHA-256 hash.
    ///
    /// If a blob with the same hash already exists, the call is
    /// idempotent — the data is identical by definition (content-
    /// addressed).  For S3 this is a natural PUT overwrite; for
    /// embedded SQLite this is handled by the caller's
    /// `ON CONFLICT` clause.
    fn put(&self, sha256: &[u8; 32], data: &[u8], meta: BlobMeta<'_>) -> Result<(), StoreError>;

    /// Retrieve compressed bytes by SHA-256 hash.
    ///
    /// Returns `None` if the blob does not exist.
    fn get(&self, sha256: &[u8; 32]) -> Result<Option<Bytes>, StoreError>;

    /// Delete compressed bytes by SHA-256 hash.
    ///
    /// Called after the metadata row's refcount reaches zero.
    /// Idempotent — deleting a non-existent blob is not an error.
    fn delete(&self, sha256: &[u8; 32]) -> Result<(), StoreError>;

    /// Check whether compressed bytes exist for the given hash.
    fn exists(&self, sha256: &[u8; 32]) -> Result<bool, StoreError>;
}
