//! # keplor-store
//!
//! KeplorDB-backed storage for the `llm_events` fact table + archive
//! manifests. Ingest via [`BatchWriter`] for amortised per-batch fsync
//! on throughput paths, or [`KdbStore::append_event_durable`] for
//! per-event sync.
//!
//! The legacy SQLite backend ([`SqliteStore`]) is **only** compiled in
//! when the `migrate-from-sqlite` feature is enabled. It exists solely
//! as a read source for the `keplor migrate-from-sqlite` subcommand;
//! it is not on any runtime ingest path. Default builds drop the
//! `rusqlite` dep entirely.

#![deny(missing_docs)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

#[cfg(feature = "s3")]
pub mod archive;
#[cfg(feature = "batch")]
pub mod batch;
pub mod error;
pub mod filter;
pub mod kdb_store;
pub mod mapping;
#[cfg(feature = "migrate-from-sqlite")]
mod migrations;
#[cfg(feature = "migrate-from-sqlite")]
pub mod store;
pub mod stored_event;
pub mod types;

#[cfg(feature = "s3")]
pub use archive::{ArchiveResult, ArchiveS3Config, Archiver};
#[cfg(feature = "batch")]
pub use batch::{BatchConfig, BatchWriter};
pub use error::StoreError;
pub use filter::{Cursor, EventFilter};
pub use kdb_store::{KdbConfig, KdbStore};
pub use stored_event::StoredEvent;
pub use types::{AggregateRow, ArchiveManifest, EventSummary, GcStats, QuotaSummary, RollupRow};

/// The legacy SQLite-backed store. Compiled only with the
/// `migrate-from-sqlite` feature; used as a read-only migration source
/// by the `keplor migrate-from-sqlite` subcommand.
#[cfg(feature = "migrate-from-sqlite")]
pub use store::Store as SqliteStore;

/// Primary event store — alias for [`KdbStore`] at the public API
/// surface so existing `keplor_store::Store` call sites stay compiling.
pub use kdb_store::KdbStore as Store;
