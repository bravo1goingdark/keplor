//! S3-compatible blob storage backend.
//!
//! Stores compressed payload blobs in any S3-compatible object store:
//! Cloudflare R2, MinIO, AWS S3, DigitalOcean Spaces, etc.
//!
//! Blobs are keyed by their SHA-256 hex string (64 chars).  PUTs are
//! naturally idempotent (same content → same key → safe overwrite),
//! giving free deduplication without coordination.
//!
//! Requires the `s3` feature flag.

use bytes::Bytes;
use s3::bucket::Bucket;
use s3::creds::Credentials;
use s3::region::Region;

use super::{BlobMeta, BlobStore};
use crate::error::StoreError;

/// Configuration for the S3 blob store.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct S3BlobConfig {
    /// Bucket name.
    pub bucket: String,
    /// S3 endpoint URL (e.g. `https://<account>.r2.cloudflarestorage.com`).
    pub endpoint: String,
    /// Region (e.g. `auto` for R2, `us-east-1` for AWS).
    pub region: String,
    /// Access key ID.
    pub access_key_id: String,
    /// Secret access key.
    pub secret_access_key: String,
    /// Optional key prefix (e.g. `blobs/`). Defaults to empty.
    #[serde(default)]
    pub prefix: String,
    /// Use path-style addressing (required for MinIO, optional for R2).
    #[serde(default)]
    pub path_style: bool,
}

/// Blob storage backed by an S3-compatible object store.
///
/// # Threading safety
///
/// All trait methods use `rt.block_on()` to bridge async S3 operations
/// into the synchronous [`super::BlobStore`] interface.  This is safe
/// because `Store` methods are always called from `tokio::task::spawn_blocking`,
/// which runs on a dedicated thread pool — never on a tokio worker thread.
/// Calling these methods directly from an async context will panic.
pub struct S3BlobStore {
    bucket: Box<Bucket>,
    prefix: String,
    /// Tokio runtime handle for running async S3 ops from sync context.
    rt: tokio::runtime::Handle,
}

impl std::fmt::Debug for S3BlobStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("S3BlobStore")
            .field("bucket", &self.bucket.name())
            .field("region", &self.bucket.region().to_string())
            .field("prefix", &self.prefix)
            .finish()
    }
}

impl S3BlobStore {
    /// Create a new S3 blob store from configuration.
    ///
    /// The caller must provide a tokio runtime handle because S3
    /// operations are async but the [`BlobStore`] trait is synchronous
    /// (called from `spawn_blocking` contexts).
    pub fn new(config: &S3BlobConfig, rt: tokio::runtime::Handle) -> Result<Self, StoreError> {
        let region =
            Region::Custom { region: config.region.clone(), endpoint: config.endpoint.clone() };

        let credentials = Credentials::new(
            Some(&config.access_key_id),
            Some(&config.secret_access_key),
            None,
            None,
            None,
        )
        .map_err(|e| StoreError::BlobS3(format!("credentials: {e}")))?;

        let mut bucket = Bucket::new(&config.bucket, region, credentials)
            .map_err(|e| StoreError::BlobS3(format!("bucket init: {e}")))?;

        if config.path_style {
            bucket = bucket.with_path_style();
        }

        Ok(Self { bucket, prefix: config.prefix.clone(), rt })
    }

    /// Build the object key for a given SHA-256 hash.
    fn key(&self, sha256: &[u8; 32]) -> String {
        let hex = crate::store::sha256_hex(sha256);
        if self.prefix.is_empty() {
            hex
        } else {
            format!("{}{hex}", self.prefix)
        }
    }

    /// Test connectivity by performing a HEAD request on the bucket.
    pub fn check_connectivity(&self) -> Result<(), StoreError> {
        self.rt
            .block_on(async { self.bucket.head_object("/").await })
            .map(|_| ())
            .map_err(|e| StoreError::BlobS3(format!("connectivity check failed: {e}")))
    }
}

impl BlobStore for S3BlobStore {
    fn put(&self, sha256: &[u8; 32], data: &[u8], _meta: BlobMeta<'_>) -> Result<(), StoreError> {
        let key = self.key(sha256);
        self.rt
            .block_on(async { self.bucket.put_object(&key, data).await })
            .map_err(|e| StoreError::BlobS3(format!("PUT {key}: {e}")))?;
        Ok(())
    }

    fn get(&self, sha256: &[u8; 32]) -> Result<Option<Bytes>, StoreError> {
        let key = self.key(sha256);
        let result = self.rt.block_on(async { self.bucket.get_object(&key).await });

        match result {
            Ok(resp) => {
                if resp.status_code() == 404 {
                    Ok(None)
                } else {
                    Ok(Some(Bytes::from(resp.to_vec())))
                }
            },
            Err(e) => {
                let msg = e.to_string();
                if msg.contains("404") || msg.contains("NoSuchKey") {
                    Ok(None)
                } else {
                    Err(StoreError::BlobS3(format!("GET {key}: {e}")))
                }
            },
        }
    }

    fn delete(&self, sha256: &[u8; 32]) -> Result<(), StoreError> {
        let key = self.key(sha256);
        self.rt
            .block_on(async { self.bucket.delete_object(&key).await })
            .map_err(|e| StoreError::BlobS3(format!("DELETE {key}: {e}")))?;
        Ok(())
    }

    fn exists(&self, sha256: &[u8; 32]) -> Result<bool, StoreError> {
        let key = self.key(sha256);
        let result = self.rt.block_on(async { self.bucket.head_object(&key).await });

        match result {
            Ok(_) => Ok(true),
            Err(e) => {
                let msg = e.to_string();
                if msg.contains("404") || msg.contains("NoSuchKey") {
                    Ok(false)
                } else {
                    Err(StoreError::BlobS3(format!("HEAD {key}: {e}")))
                }
            },
        }
    }
}
