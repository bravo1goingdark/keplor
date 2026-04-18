//! Embedded blob storage — blobs live in the SQLite `payload_blobs.data`
//! column alongside their metadata.
//!
//! This is the zero-dependency default.  When this backend is active,
//! [`crate::store::Store`] writes blob data directly into the
//! `payload_blobs` table via its existing `INSERT ... ON CONFLICT`
//! statements.  The trait methods here provide the read/delete/exists
//! interface that the `Store` uses for operations that go through the
//! [`super::BlobStore`] abstraction.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use bytes::Bytes;
use rusqlite::{params, Connection, OptionalExtension};

use super::{BlobMeta, BlobStore};
use crate::error::StoreError;

/// Blob storage backed by the SQLite `payload_blobs` table.
///
/// Shares the same database file as the event store.  For the embedded
/// backend, `put` writes directly into `payload_blobs.data`; for the
/// S3 backend that column is `NULL` and data lives externally.
pub struct EmbeddedBlobStore {
    /// Read connections — shared with the parent Store's pool.
    read_pool: Vec<Mutex<Connection>>,
    read_idx: AtomicUsize,
    /// Write connection — shared with the parent Store.
    write_conn: Mutex<Connection>,
}

impl std::fmt::Debug for EmbeddedBlobStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EmbeddedBlobStore")
            .field("read_pool_size", &self.read_pool.len())
            .finish_non_exhaustive()
    }
}

impl EmbeddedBlobStore {
    /// Create a new embedded blob store.
    ///
    /// In the embedded case, the `Store` handles blob writes inline in
    /// its transactions.  This struct provides read/delete/exists for
    /// code paths that go through the `BlobStore` trait.
    ///
    /// # Note
    ///
    /// For the embedded backend, `put` is typically not called — the
    /// `Store` writes blobs directly in its batch transaction for
    /// atomicity.  The `put` method is provided for trait completeness
    /// and testing.
    pub fn new(write_conn: Mutex<Connection>, read_pool: Vec<Mutex<Connection>>) -> Self {
        Self { write_conn, read_pool, read_idx: AtomicUsize::new(0) }
    }

    /// Acquire a read connection from the pool (round-robin).
    fn read_conn(&self) -> Result<std::sync::MutexGuard<'_, Connection>, StoreError> {
        let idx = self.read_idx.fetch_add(1, Ordering::Relaxed) % self.read_pool.len();
        self.read_pool[idx].lock().map_err(|e| StoreError::LockPoisoned(e.to_string()))
    }

    /// Acquire the write connection.
    fn write_conn(&self) -> Result<std::sync::MutexGuard<'_, Connection>, StoreError> {
        self.write_conn.lock().map_err(|e| StoreError::LockPoisoned(e.to_string()))
    }
}

impl BlobStore for EmbeddedBlobStore {
    fn put(&self, sha256: &[u8; 32], data: &[u8], meta: BlobMeta<'_>) -> Result<(), StoreError> {
        let conn = self.write_conn()?;
        conn.execute(
            "INSERT INTO payload_blobs(
                sha256, component_type, provider, compression, dict_id,
                uncompressed_size, compressed_size, refcount, hit_count,
                data, first_seen_at
             ) VALUES(?1, ?2, ?3, 'zstd_raw', NULL, ?4, ?5, 1, 0, ?6, strftime('%s','now'))
             ON CONFLICT(sha256) DO UPDATE SET
                refcount = refcount + 1,
                hit_count = hit_count + 1",
            params![
                &sha256[..],
                meta.component_type,
                meta.provider,
                data.len() as i64,
                data.len() as i64,
                data,
            ],
        )?;
        Ok(())
    }

    fn get(&self, sha256: &[u8; 32]) -> Result<Option<Bytes>, StoreError> {
        let conn = self.read_conn()?;
        let result: Option<Vec<u8>> = conn
            .query_row("SELECT data FROM payload_blobs WHERE sha256 = ?1", [&sha256[..]], |r| {
                r.get(0)
            })
            .optional()?;
        Ok(result.map(Bytes::from))
    }

    fn delete(&self, sha256: &[u8; 32]) -> Result<(), StoreError> {
        let conn = self.write_conn()?;
        conn.execute("DELETE FROM payload_blobs WHERE sha256 = ?1", [&sha256[..]])?;
        Ok(())
    }

    fn exists(&self, sha256: &[u8; 32]) -> Result<bool, StoreError> {
        let conn = self.read_conn()?;
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM payload_blobs WHERE sha256 = ?1",
            [&sha256[..]],
            |r| r.get(0),
        )?;
        Ok(count > 0)
    }
}
