//! Storage-layer DTOs returned by both [`KdbStore`](crate::kdb_store::KdbStore)
//! and the legacy [`SqliteStore`](crate::SqliteStore) feature-gated migration
//! source. Defining them here (rather than inside `store.rs`) lets the SQLite
//! impl be gated behind the `migrate-from-sqlite` feature without these
//! types disappearing from the runtime API.

/// Manifest row for an archived event chunk in S3/R2.
#[derive(Debug, Clone, Default)]
pub struct ArchiveManifest {
    /// Unique archive chunk id (ULID).
    pub archive_id: String,
    /// User id that owns these events (`"_none"` if absent).
    pub user_id: String,
    /// Day partition as `YYYY-MM-DD`.
    pub day: String,
    /// Full S3 key.
    pub s3_key: String,
    /// Number of events in this chunk.
    pub event_count: usize,
    /// Earliest event timestamp (nanoseconds).
    pub min_ts_ns: i64,
    /// Latest event timestamp (nanoseconds).
    pub max_ts_ns: i64,
    /// Compressed file size in bytes.
    pub compressed_bytes: usize,
    /// When this archive was created (epoch seconds).
    pub created_at: i64,
}

/// Statistics returned by GC operations.
#[derive(Debug, Clone, Default)]
pub struct GcStats {
    /// Number of event rows deleted.
    pub events_deleted: usize,
    /// Number of blob rows deleted (refcount reached 0).
    pub blobs_deleted: usize,
}

/// Lightweight event projection for the HTTP API.
#[derive(Debug, Clone)]
pub struct EventSummary {
    /// Primary key — time-sortable ULID.
    pub id: keplor_core::EventId,
    /// Wall-clock capture time in nanoseconds.
    pub ts_ns: i64,
    /// Caller-provided user id.
    pub user_id: Option<String>,
    /// API key id.
    pub api_key_id: Option<String>,
    /// Provider id key (e.g. `"openai"`).
    pub provider: String,
    /// Model name.
    pub model: String,
    /// Request endpoint.
    pub endpoint: String,
    /// HTTP status code.
    pub http_status: Option<u16>,
    /// Input tokens.
    pub input_tokens: u32,
    /// Output tokens.
    pub output_tokens: u32,
    /// Cache-read input tokens.
    pub cache_read_input_tokens: u32,
    /// Cache-creation input tokens.
    pub cache_creation_input_tokens: u32,
    /// Reasoning tokens.
    pub reasoning_tokens: u32,
    /// Cost in nanodollars.
    pub cost_nanodollars: i64,
    /// Time to first token (ms).
    pub ttft_ms: Option<u32>,
    /// Total latency (ms).
    pub total_ms: u32,
    /// Whether the request was streaming.
    pub streaming: bool,
    /// Ingestion source.
    pub source: Option<String>,
    /// Error type (e.g. `"rate_limited"`, `"upstream_429"`).
    pub error_type: Option<String>,
    /// Arbitrary metadata as JSON text.
    pub metadata_json: Option<String>,
}

/// Cost + event count from a quota query.
#[derive(Debug, Clone)]
pub struct QuotaSummary {
    /// Total cost in nanodollars.
    pub cost_nanodollars: i64,
    /// Number of events matching the filter.
    pub event_count: i64,
}

/// A single row from the daily rollups table.
#[derive(Debug, Clone)]
pub struct RollupRow {
    /// Day boundary as epoch seconds.
    pub day: i64,
    /// User id (empty string if not set).
    pub user_id: String,
    /// API key id (empty string if not set).
    pub api_key_id: String,
    /// Provider id key.
    pub provider: String,
    /// Model name.
    pub model: String,
    /// Number of events.
    pub event_count: i64,
    /// Number of events with http_status >= 400.
    pub error_count: i64,
    /// Total input tokens.
    pub input_tokens: i64,
    /// Total output tokens.
    pub output_tokens: i64,
    /// Total cache-read input tokens.
    pub cache_read_input_tokens: i64,
    /// Total cache-creation input tokens.
    pub cache_creation_input_tokens: i64,
    /// Total cost in nanodollars.
    pub cost_nanodollars: i64,
}

/// An aggregated stats row (optionally grouped by model).
#[derive(Debug, Clone)]
pub struct AggregateRow {
    /// Provider (empty string when not grouped).
    pub provider: String,
    /// Model name (empty string when not grouped).
    pub model: String,
    /// Number of events.
    pub event_count: i64,
    /// Number of error events.
    pub error_count: i64,
    /// Total input tokens.
    pub input_tokens: i64,
    /// Total output tokens.
    pub output_tokens: i64,
    /// Total cache-read input tokens.
    pub cache_read_input_tokens: i64,
    /// Total cache-creation input tokens.
    pub cache_creation_input_tokens: i64,
    /// Total cost in nanodollars.
    pub cost_nanodollars: i64,
}
