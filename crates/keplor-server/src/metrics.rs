//! Centralised metric name + label key constants.
//!
//! All Prometheus metric names and label keys emitted by keplor live
//! here so they're not stringly-typed across the codebase. Keeping them
//! in one place makes it impossible to accidentally fork a metric name
//! through a typo (`keplor_gc_segements_deleted_total`) and lets callers
//! reason about cardinality at a glance.
//!
//! ## Cardinality
//!
//! Label values must come from a small, server-controlled set — never
//! from user-supplied strings. The `error_type` label is bounded by
//! the variant count of [`crate::error::ServerError`] (currently 7,
//! cap 10). The `tier` label is bounded by configured retention tiers
//! (typically 3-5). The `provider` label is bounded by
//! [`keplor_core::Provider`] variants (~10). The `model` label is
//! intentionally accepted unbounded — it's already normalised through
//! the pricing catalog's known-models set, so its cardinality tracks
//! the catalog (~200), not arbitrary client input.

// ── Metric names ────────────────────────────────────────────────────

/// Per-tier ingest latency histogram (seconds), labels:
/// `{tier, provider, model}`.
///
/// Recorded around the durable `BatchWriter::write` flush to capture
/// end-to-end pipeline + storage latency. Complements (does not
/// replace) the legacy `keplor_ingest_duration_seconds`.
pub const INGEST_LATENCY_SECONDS: &str = "keplor_ingest_latency_seconds";

/// Current depth of the batch writer's bounded channel.
pub const BATCH_QUEUE_DEPTH: &str = "keplor_batch_queue_depth";

/// Capacity of the batch writer's bounded channel.
pub const BATCH_QUEUE_CAPACITY: &str = "keplor_batch_queue_capacity";

/// Segments unlinked by GC, labels: `{tier}`.
pub const GC_SEGMENTS_DELETED_TOTAL: &str = "keplor_gc_segments_deleted_total";

/// Bytes freed by GC segment unlinks, labels: `{tier}`.
pub const GC_BYTES_FREED_TOTAL: &str = "keplor_gc_bytes_freed_total";

/// Archive chunk attempts, labels: `{status}` where status is
/// `"success"` or `"fail"`.
pub const ARCHIVE_CHUNKS_TOTAL: &str = "keplor_archive_chunks_total";

/// Compressed bytes uploaded to object storage by the archiver.
pub const ARCHIVE_BYTES_UPLOADED_TOTAL: &str = "keplor_archive_bytes_uploaded_total";

// ── Label keys ──────────────────────────────────────────────────────

pub const LABEL_TIER: &str = "tier";
pub const LABEL_PROVIDER: &str = "provider";
pub const LABEL_MODEL: &str = "model";
pub const LABEL_STAGE: &str = "stage";
pub const LABEL_ERROR_TYPE: &str = "error_type";
pub const LABEL_STATUS: &str = "status";

// ── Label values ────────────────────────────────────────────────────

pub const STATUS_SUCCESS: &str = "success";
pub const STATUS_FAIL: &str = "fail";

/// Map a [`crate::error::ServerError`] variant to a low-cardinality
/// label value. Variant names only — never the inner Display string,
/// which contains user-supplied data.
///
/// Cardinality: bounded by enum variants (currently 7, cap 10).
pub fn error_type_label(err: &crate::error::ServerError) -> &'static str {
    use crate::error::ServerError;
    match err {
        ServerError::Validation(_) => "validation",
        ServerError::UnknownProvider(_) => "unknown_provider",
        ServerError::InvalidTimestamp(_) => "invalid_timestamp",
        ServerError::Store(_) => "store",
        ServerError::Json(_) => "json",
        ServerError::StorageFull(_) => "storage_full",
        ServerError::Internal(_) => "internal",
    }
}
