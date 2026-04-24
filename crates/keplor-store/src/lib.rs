//! # keplor-store
//!
//! KeplorDB-backed storage for the `llm_events` fact table + archive
//! manifests. Ingest via [`BatchWriter`] for amortised per-batch fsync
//! on throughput paths, or [`KdbStore::append_event_durable`] for
//! per-event sync.
//!
//! The legacy SQLite backend ([`SqliteStore`]) is retained only as a
//! migration source — the `keplor migrate-from-sqlite` subcommand
//! opens both sides and copies events across. It is **not** on any
//! runtime ingest path.

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
mod migrations;
pub mod store;
pub mod stored_event;

#[cfg(feature = "s3")]
pub use archive::{ArchiveResult, ArchiveS3Config, Archiver};
#[cfg(feature = "batch")]
pub use batch::{BatchConfig, BatchWriter};
pub use error::StoreError;
pub use filter::{Cursor, EventFilter};
pub use kdb_store::{KdbConfig, KdbStore};
pub use store::{AggregateRow, ArchiveManifest, EventSummary, GcStats, QuotaSummary, RollupRow};
pub use stored_event::StoredEvent;

/// The legacy SQLite-backed store. Retained as a read-only migration
/// source (`keplor migrate-from-sqlite`); no runtime path writes to it.
pub use store::Store as SqliteStore;

/// Primary event store — alias for [`KdbStore`] at the public API
/// surface so existing `keplor_store::Store` call sites stay compiling.
pub use kdb_store::KdbStore as Store;
