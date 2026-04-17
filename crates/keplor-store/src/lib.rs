//! # keplor-store
//!
//! Local-first storage: the `llm_events` fact table, the content-addressed
//! `payload_blobs` table, and the zstd compression engine.
//!
//! For high-throughput ingestion, use [`BatchWriter`] which accumulates
//! events and flushes in bulk transactions (amortising `BEGIN`/`COMMIT`
//! overhead).  For single-event writes, use [`Store::append_event`].

#![deny(missing_docs)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

#[cfg(feature = "batch")]
pub mod batch;
pub mod components;
pub mod compress;
pub mod error;
pub mod filter;
mod migrations;
pub mod store;

#[cfg(feature = "batch")]
pub use batch::{BatchConfig, BatchWriter};
pub use components::ComponentType;
pub use compress::ZstdCoder;
pub use error::StoreError;
pub use filter::{Cursor, EventFilter};
pub use store::{AggregateRow, EventSummary, GcStats, QuotaSummary, RollupRow, Store};
