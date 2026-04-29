//! # keplor-store
//!
//! KeplorDB-backed storage for the `llm_events` fact table + archive
//! manifests. Ingest via [`BatchWriter`] for amortised per-batch fsync
//! on throughput paths, or [`KdbStore::append_event_durable`] for
//! per-event sync.

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
pub mod stored_event;
pub mod types;

#[cfg(feature = "s3")]
pub use archive::{ArchiveResult, ArchiveS3Config, Archiver};
#[cfg(feature = "batch")]
pub use batch::{BatchConfig, BatchWriter};
pub use error::StoreError;
pub use filter::{Cursor, EventFilter};
pub use kdb_store::{KdbConfig, KdbStore, TierEngineStats};
pub use stored_event::StoredEvent;
pub use types::{AggregateRow, ArchiveManifest, EventSummary, GcStats, QuotaSummary, RollupRow};

/// Primary event store — alias for [`KdbStore`] at the public API
/// surface so existing `keplor_store::Store` call sites stay compiling.
pub use kdb_store::KdbStore as Store;
