//! # keplor-store
//!
//! Local-first storage: the `llm_events` fact table, the content-addressed
//! `payload_blobs` table, and the zstd compression engine.
//!
//! For high-throughput ingestion, use [`BatchWriter`] which accumulates
//! events and flushes in bulk transactions (amortising `BEGIN`/`COMMIT`
//! overhead).  For single-event writes, use [`Store::append_event`].
//!
//! ## Blob storage backends
//!
//! The [`blob::BlobStore`] trait abstracts where compressed payload bytes
//! live.  The default [`blob::EmbeddedBlobStore`] keeps them in SQLite;
//! the optional [`blob::S3BlobStore`] (feature `s3`) stores them in any
//! S3-compatible service (Cloudflare R2, MinIO, AWS S3).

#![deny(missing_docs)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

#[cfg(feature = "batch")]
pub mod batch;
pub mod blob;
pub mod components;
pub mod compress;
pub mod error;
pub mod filter;
mod migrations;
pub mod store;

#[cfg(feature = "batch")]
pub use batch::{BatchConfig, BatchWriter};
#[cfg(feature = "s3")]
pub use blob::S3BlobStore;
pub use blob::{BlobMeta, BlobStore, EmbeddedBlobStore};
pub use components::ComponentType;
pub use compress::ZstdCoder;
pub use error::StoreError;
pub use filter::{Cursor, EventFilter};
pub use store::{AggregateRow, EventSummary, GcStats, QuotaSummary, RollupRow, Store};
