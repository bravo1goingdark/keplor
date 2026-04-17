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
    /// Only events on or after this timestamp (nanoseconds).
    pub from_ts_ns: Option<i64>,
    /// Only events on or before this timestamp (nanoseconds).
    pub to_ts_ns: Option<i64>,
}
