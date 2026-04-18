//! S3-compatible blob storage backend.
//!
//! Stores compressed payload blobs in any S3-compatible object store:
//! Cloudflare R2, MinIO, AWS S3, DigitalOcean Spaces, etc.
//!
//! Uses the `object_store` crate (Apache Arrow project) which relies on
//! `aws-lc-rs` for TLS — no `ring` dependency.
//!
//! Blobs are keyed by their SHA-256 hex string (64 chars).  PUTs are
//! naturally idempotent (same content → same key → safe overwrite),
//! giving free deduplication without coordination.
//!
//! Requires the `s3` feature flag.

use std::sync::Arc;

use bytes::Bytes;
use object_store::aws::AmazonS3Builder;
use object_store::path::Path as ObjectPath;
use object_store::{ObjectStore, ObjectStoreExt};

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
    client: Arc<dyn ObjectStore>,
    prefix: String,
    /// Tokio runtime handle for running async S3 ops from sync context.
    rt: tokio::runtime::Handle,
}

impl std::fmt::Debug for S3BlobStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("S3BlobStore").field("prefix", &self.prefix).finish_non_exhaustive()
    }
}

impl S3BlobStore {
    /// Create a new S3 blob store from configuration.
    ///
    /// The caller must provide a tokio runtime handle because S3
    /// operations are async but the [`BlobStore`] trait is synchronous
    /// (called from `spawn_blocking` contexts).
    pub fn new(config: &S3BlobConfig, rt: tokio::runtime::Handle) -> Result<Self, StoreError> {
        let mut builder = AmazonS3Builder::new()
            .with_bucket_name(&config.bucket)
            .with_endpoint(&config.endpoint)
            .with_region(&config.region)
            .with_access_key_id(&config.access_key_id)
            .with_secret_access_key(&config.secret_access_key)
            .with_allow_http(config.endpoint.starts_with("http://"));

        if config.path_style {
            builder = builder.with_virtual_hosted_style_request(false);
        }

        let client =
            builder.build().map_err(|e| StoreError::BlobS3(format!("S3 client init: {e}")))?;

        Ok(Self { client: Arc::new(client), prefix: config.prefix.clone(), rt })
    }

    /// Build the object path for a given SHA-256 hash.
    fn path(&self, sha256: &[u8; 32]) -> ObjectPath {
        let hex = crate::store::sha256_hex(sha256);
        if self.prefix.is_empty() {
            ObjectPath::from(hex)
        } else {
            ObjectPath::from(format!("{}{hex}", self.prefix))
        }
    }

    /// Test connectivity by attempting a HEAD on a probe key.
    ///
    /// A `NotFound` error is expected and counts as success (the bucket
    /// is reachable). Any other error indicates a connectivity problem.
    pub fn check_connectivity(&self) -> Result<(), StoreError> {
        let probe = ObjectPath::from("__keplor_probe__");
        let result = self.rt.block_on(async { self.client.head(&probe).await });
        match result {
            Ok(_) | Err(object_store::Error::NotFound { .. }) => Ok(()),
            Err(e) => Err(StoreError::BlobS3(format!("connectivity check failed: {e}"))),
        }
    }
}

impl BlobStore for S3BlobStore {
    fn put(&self, sha256: &[u8; 32], data: &[u8], _meta: BlobMeta<'_>) -> Result<(), StoreError> {
        let path = self.path(sha256);
        let payload = Bytes::copy_from_slice(data);
        self.rt
            .block_on(async { self.client.put(&path, payload.into()).await })
            .map_err(|e| StoreError::BlobS3(format!("PUT {path}: {e}")))?;
        Ok(())
    }

    fn get(&self, sha256: &[u8; 32]) -> Result<Option<Bytes>, StoreError> {
        let path = self.path(sha256);
        let result = self.rt.block_on(async { self.client.get(&path).await });

        match result {
            Ok(resp) => {
                let data = self
                    .rt
                    .block_on(async { resp.bytes().await })
                    .map_err(|e| StoreError::BlobS3(format!("GET read {path}: {e}")))?;
                Ok(Some(data))
            },
            Err(object_store::Error::NotFound { .. }) => Ok(None),
            Err(e) => Err(StoreError::BlobS3(format!("GET {path}: {e}"))),
        }
    }

    fn delete(&self, sha256: &[u8; 32]) -> Result<(), StoreError> {
        let path = self.path(sha256);
        self.rt
            .block_on(async { self.client.delete(&path).await })
            .map_err(|e| StoreError::BlobS3(format!("DELETE {path}: {e}")))?;
        Ok(())
    }

    fn exists(&self, sha256: &[u8; 32]) -> Result<bool, StoreError> {
        let path = self.path(sha256);
        let result = self.rt.block_on(async { self.client.head(&path).await });

        match result {
            Ok(_) => Ok(true),
            Err(object_store::Error::NotFound { .. }) => Ok(false),
            Err(e) => Err(StoreError::BlobS3(format!("HEAD {path}: {e}"))),
        }
    }
}
