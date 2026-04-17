//! Query filters and cursor-based pagination for [`crate::Store::query`].

use smol_str::SmolStr;

/// Opaque pagination cursor (the `ts_ns` of the last row returned).
#[derive(Debug, Clone, Copy)]
pub struct Cursor(pub i64);

/// Filter predicate for [`crate::Store::query`].
#[derive(Debug, Clone, Default)]
pub struct EventFilter {
    /// Only events from this user.
    pub user_id: Option<SmolStr>,
    /// Only events from this API key.
    pub api_key_id: Option<SmolStr>,
    /// Only events for this model.
    pub model: Option<SmolStr>,
    /// Only events from this provider.
    pub provider: Option<SmolStr>,
    /// Only events from this ingestion source.
    pub source: Option<SmolStr>,
    /// Only events on or after this timestamp (nanoseconds).
    pub from_ts_ns: Option<i64>,
    /// Only events on or before this timestamp (nanoseconds).
    pub to_ts_ns: Option<i64>,
    /// Only events with http_status >= this value.
    pub http_status_min: Option<u16>,
    /// Only events with http_status < this value.
    pub http_status_max: Option<u16>,
    /// Only events whose `metadata_json` contains this value at `$.user_tag`.
    pub meta_user_tag: Option<SmolStr>,
    /// Only events whose `metadata_json` contains this value at `$.session_tag`.
    pub meta_session_tag: Option<SmolStr>,
}
