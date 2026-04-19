//! # keplor-store
//!
//! Local-first storage: the `llm_events` fact table and `daily_rollups`
//! aggregation.
//!
//! For high-throughput ingestion, use [`BatchWriter`] which accumulates
//! events and flushes in bulk transactions (amortising `BEGIN`/`COMMIT`
//! overhead).  For single-event writes, use [`Store::append_event`].

#![deny(missing_docs)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

#[cfg(feature = "s3")]
pub mod archive;
#[cfg(feature = "batch")]
pub mod batch;
pub mod error;
pub mod filter;
mod migrations;
pub mod store;
pub mod stored_event;

#[cfg(feature = "s3")]
pub use archive::{ArchiveResult, ArchiveS3Config, Archiver};
#[cfg(feature = "batch")]
pub use batch::{BatchConfig, BatchWriter};
pub use error::StoreError;
pub use filter::{Cursor, EventFilter};
pub use store::{
    AggregateRow, ArchiveManifest, EventSummary, GcStats, QuotaSummary, RollupRow, Store,
};
pub use stored_event::StoredEvent;
