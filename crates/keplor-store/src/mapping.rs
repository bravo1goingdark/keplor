//! `LlmEvent` ↔ `keplordb::LogEvent` mapping — the schema that fixes
//! keplor's row shape into KeplorDB's positional columnar layout.
//!
//! The dim / counter / label indices are **declared once here and never
//! reordered**: the column positions are part of the on-disk segment
//! format, so any change is a schema migration.  Every segment is
//! written under `SCHEMA_ID` and KeplorDB refuses to open a segment
//! with a mismatched id.
//!
//! ## Design choices
//!
//! - `Option<T>` scalars (`http_status`, `ttft_ms`, `time_to_close_ms`)
//!   are packed as a sentinel value (`0`) plus a presence bit in the
//!   flags field, so the information survives the round trip.
//! - `Provider::OpenAICompatible { base_url }` spills the `base_url`
//!   into a label so the dim stays a small, stable set of strings that
//!   bloom/zone-map cheaply.
//! - `ProviderError` is serialised with `serde_json` — it's already
//!   `Serialize`/`Deserialize`, and storing the JSON keeps the variant
//!   discriminator + payload together for a clean round trip.
//! - `TraceId` (`[u8;16]`) is hex-encoded into a label; `IpAddr` uses
//!   its display form.

use std::net::IpAddr;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use http::Method;
use keplor_core::{
    ApiKeyId, EventFlags, EventId, Latencies, LlmEvent, OrgId, ProjectId, Provider, ProviderError,
    RouteId, TraceId, Usage, UserId,
};
use keplordb::{EventRef, LogEvent};
use serde_json::Value as JsonValue;
use smol_str::SmolStr;

use crate::filter::{Cursor, EventFilter};

/// Number of indexed dimensions.  **Do not reorder — this is part of
/// the on-disk segment format.**
pub const D: usize = 14;
/// Number of u32 counters.
pub const C: usize = 13;
/// Number of string labels (stored, not indexed).
pub const L: usize = 8;

/// Schema id written into every segment header.  Bump on any dim /
/// counter / label reshape.
pub const SCHEMA_ID: u8 = 1;

// ── Dimension indices (indexed, interned, filterable) ─────────────────

/// `user_id` — primary bloom filter lives here; drives rollup key.
pub const DIM_USER_ID: usize = 0;
/// `api_key_id` — rollup key.
pub const DIM_API_KEY_ID: usize = 1;
/// `model` — rollup key, used by stats aggregation.
pub const DIM_MODEL: usize = 2;
/// Provider id-key string (`"openai"`, `"anthropic"`, ...).
pub const DIM_PROVIDER: usize = 3;
/// Ingestion source string (`"litellm"`, `"custom-gateway"`, ...).
pub const DIM_SOURCE: usize = 4;
/// `org_id`.
pub const DIM_ORG_ID: usize = 5;
/// `project_id`.
pub const DIM_PROJECT_ID: usize = 6;
/// `route_id`.
pub const DIM_ROUTE_ID: usize = 7;
/// `model_family`.
pub const DIM_MODEL_FAMILY: usize = 8;
/// Request endpoint path.
pub const DIM_ENDPOINT: usize = 9;
/// Retention tier (`"free"`, `"pro"`, `"team"`).
pub const DIM_TIER: usize = 10;
/// Stable error-type discriminator (`""` when no error).
pub const DIM_ERROR_TYPE: usize = 11;
/// Event `metadata.user_tag` field, promoted to a first-class dim so
/// filtered queries skip segments instead of scanning JSON.
pub const DIM_USER_TAG: usize = 12;
/// Event `metadata.session_tag` field, same motivation as
/// [`DIM_USER_TAG`].
pub const DIM_SESSION_TAG: usize = 13;

// ── Counter indices (u32) ─────────────────────────────────────────────

/// Prompt / input tokens.
pub const COUNTER_INPUT_TOKENS: usize = 0;
/// Completion / output tokens.
pub const COUNTER_OUTPUT_TOKENS: usize = 1;
/// Cache-read input tokens.
pub const COUNTER_CACHE_READ: usize = 2;
/// Cache-creation input tokens.
pub const COUNTER_CACHE_CREATION: usize = 3;
/// Reasoning / thinking tokens.
pub const COUNTER_REASONING: usize = 4;
/// Audio input tokens.
pub const COUNTER_AUDIO_IN: usize = 5;
/// Audio output tokens.
pub const COUNTER_AUDIO_OUT: usize = 6;
/// Image input tokens.
pub const COUNTER_IMAGE: usize = 7;
/// Video input seconds (Gemini).
pub const COUNTER_VIDEO_SECONDS: usize = 8;
/// Tool-use tokens.
pub const COUNTER_TOOL_USE: usize = 9;
/// Billable search queries.
pub const COUNTER_SEARCH_QUERIES: usize = 10;
/// `time_to_close_ms` when present — zero otherwise; paired with
/// [`FLAG_HAS_CLOSE_MS`].
pub const COUNTER_CLOSE_MS: usize = 11;
/// `0` for success, `1` for errored events — lets stats aggregation
/// compute `error_count` as a simple saturating sum.
pub const COUNTER_IS_ERROR: usize = 12;

// ── Label indices (stored, not indexed) ───────────────────────────────

/// JSON-encoded `ProviderError` payload (empty when no error).
pub const LABEL_ERROR_JSON: usize = 0;
/// Upstream provider request id.
pub const LABEL_REQUEST_ID: usize = 1;
/// 32-char lowercase hex W3C trace id.
pub const LABEL_TRACE_ID: usize = 2;
/// Client IP, in its display form.
pub const LABEL_CLIENT_IP: usize = 3;
/// Client `User-Agent` header.
pub const LABEL_USER_AGENT: usize = 4;
/// Free-form metadata JSON (raw, not parsed).
pub const LABEL_METADATA_JSON: usize = 5;
/// HTTP method verb (`"POST"`, `"GET"`, ...).
pub const LABEL_METHOD: usize = 6;
/// Base URL for `Provider::OpenAICompatible` (empty for other variants).
pub const LABEL_PROVIDER_VARIANT: usize = 7;

// ── Flag bit positions (u16) ──────────────────────────────────────────

/// Response body was streamed.
pub const FLAG_STREAMING: u16 = 1 << 0;
/// Response contains tool calls.
pub const FLAG_TOOL_CALLS: u16 = 1 << 1;
/// Response contains reasoning tokens.
pub const FLAG_REASONING: u16 = 1 << 2;
/// Stream ended prematurely.
pub const FLAG_STREAM_INCOMPLETE: u16 = 1 << 3;
/// Provider cache was used.
pub const FLAG_CACHED_USED: u16 = 1 << 4;
/// Request was blocked by a server-side budget rule.
pub const FLAG_BUDGET_BLOCKED: u16 = 1 << 5;
/// `ttft_ms` is present (distinguishes `Some(0)` from `None`).
pub const FLAG_HAS_TTFT: u16 = 1 << 8;
/// `http_status` is present.
pub const FLAG_HAS_HTTP_STATUS: u16 = 1 << 9;
/// `error` is present.
pub const FLAG_HAS_ERROR: u16 = 1 << 10;
/// `time_to_close_ms` is present.
pub const FLAG_HAS_CLOSE_MS: u16 = 1 << 11;

/// Errors returned when converting a stored [`EventRef`] back into an
/// [`LlmEvent`].
#[derive(Debug, thiserror::Error)]
pub enum MappingError {
    /// Event id parsing failed.
    #[error("invalid event id: {0}")]
    InvalidEventId(String),
    /// A stored trace id was not 32 hex characters.
    #[error("invalid trace id: {0}")]
    InvalidTraceId(String),
    /// A stored IP string did not parse.
    #[error("invalid client ip: {0}")]
    InvalidIp(String),
    /// The stored error JSON did not deserialise.
    #[error("invalid error payload: {0}")]
    InvalidError(String),
    /// The stored metadata was not valid JSON.
    #[error("invalid metadata: {0}")]
    InvalidMetadata(String),
    /// The stored HTTP method string was not parseable.
    #[error("invalid http method: {0}")]
    InvalidMethod(String),
}

/// Convert an [`LlmEvent`] into the KeplorDB row it persists as.
#[must_use]
pub fn to_log_event(event: &LlmEvent) -> LogEvent<D, C, L> {
    let mut dims: [String; D] = std::array::from_fn(|_| String::new());
    dims[DIM_USER_ID] = event.user_id.as_ref().map(|u| u.as_str().to_owned()).unwrap_or_default();
    dims[DIM_API_KEY_ID] =
        event.api_key_id.as_ref().map(|k| k.as_str().to_owned()).unwrap_or_default();
    dims[DIM_MODEL] = event.model.as_str().to_owned();
    dims[DIM_PROVIDER] = event.provider.id_key().to_owned();
    dims[DIM_SOURCE] = event.source.as_ref().map(|s| s.as_str().to_owned()).unwrap_or_default();
    dims[DIM_ORG_ID] = event.org_id.as_ref().map(|o| o.as_str().to_owned()).unwrap_or_default();
    dims[DIM_PROJECT_ID] =
        event.project_id.as_ref().map(|p| p.as_str().to_owned()).unwrap_or_default();
    dims[DIM_ROUTE_ID] = event.route_id.as_str().to_owned();
    dims[DIM_MODEL_FAMILY] =
        event.model_family.as_ref().map(|f| f.as_str().to_owned()).unwrap_or_default();
    dims[DIM_ENDPOINT] = event.endpoint.as_str().to_owned();
    dims[DIM_TIER] = event.tier.as_str().to_owned();
    dims[DIM_ERROR_TYPE] =
        event.error.as_ref().map(provider_error_type_key).unwrap_or_default().to_owned();
    let (user_tag, session_tag) = extract_tags(event.metadata.as_ref());
    dims[DIM_USER_TAG] = user_tag;
    dims[DIM_SESSION_TAG] = session_tag;

    let mut counters: [u32; C] = [0; C];
    counters[COUNTER_INPUT_TOKENS] = event.usage.input_tokens;
    counters[COUNTER_OUTPUT_TOKENS] = event.usage.output_tokens;
    counters[COUNTER_CACHE_READ] = event.usage.cache_read_input_tokens;
    counters[COUNTER_CACHE_CREATION] = event.usage.cache_creation_input_tokens;
    counters[COUNTER_REASONING] = event.usage.reasoning_tokens;
    counters[COUNTER_AUDIO_IN] = event.usage.audio_input_tokens;
    counters[COUNTER_AUDIO_OUT] = event.usage.audio_output_tokens;
    counters[COUNTER_IMAGE] = event.usage.image_tokens;
    counters[COUNTER_VIDEO_SECONDS] = event.usage.video_seconds;
    counters[COUNTER_TOOL_USE] = event.usage.tool_use_tokens;
    counters[COUNTER_SEARCH_QUERIES] = event.usage.search_queries;
    counters[COUNTER_CLOSE_MS] = event.latency.time_to_close_ms.unwrap_or(0);
    counters[COUNTER_IS_ERROR] = u32::from(event.error.is_some());

    let mut labels: [String; L] = std::array::from_fn(|_| String::new());
    labels[LABEL_ERROR_JSON] = match event.error.as_ref() {
        Some(e) => encode_error(e),
        None => String::new(),
    };
    labels[LABEL_REQUEST_ID] = event.request_id.as_ref().map(|r| r.to_string()).unwrap_or_default();
    labels[LABEL_TRACE_ID] = event.trace_id.as_ref().map(trace_id_to_hex).unwrap_or_default();
    labels[LABEL_CLIENT_IP] = event.client_ip.map(|ip| ip.to_string()).unwrap_or_default();
    labels[LABEL_USER_AGENT] = event.user_agent.as_ref().map(|u| u.to_string()).unwrap_or_default();
    labels[LABEL_METADATA_JSON] = match event.metadata.as_ref() {
        Some(v) => serde_json::to_string(v).unwrap_or_default(),
        None => String::new(),
    };
    labels[LABEL_METHOD] = event.method.as_str().to_owned();
    labels[LABEL_PROVIDER_VARIANT] = match &event.provider {
        Provider::OpenAICompatible { base_url } => base_url.as_ref().to_owned(),
        _ => String::new(),
    };

    LogEvent {
        id: event.id.to_string(),
        ts_ns: event.ts_ns,
        metric: event.cost_nanodollars,
        counters,
        latency_ms: event.latency.total_ms,
        latency_detail_ms: event.latency.ttft_ms.unwrap_or(0),
        status: event.http_status.unwrap_or(0),
        flags: keplordb::EventFlags(pack_flags(event)),
        dims,
        labels,
    }
}

/// Convert a stored [`EventRef`] back into an [`LlmEvent`].
///
/// This is the reverse of [`to_log_event`] and is used by query paths.
pub fn from_event_ref(r: &EventRef<D, C, L>) -> Result<LlmEvent, MappingError> {
    let id: EventId = r.id.parse().map_err(|_| MappingError::InvalidEventId(r.id.clone()))?;

    let provider = read_provider(&r.dims[DIM_PROVIDER], &r.labels[LABEL_PROVIDER_VARIANT]);

    let flags_u16 = r.flags.0;
    let has_ttft = flags_u16 & FLAG_HAS_TTFT != 0;
    let has_http_status = flags_u16 & FLAG_HAS_HTTP_STATUS != 0;
    let has_error = flags_u16 & FLAG_HAS_ERROR != 0;
    let has_close_ms = flags_u16 & FLAG_HAS_CLOSE_MS != 0;

    let error = if has_error && !r.labels[LABEL_ERROR_JSON].is_empty() {
        Some(decode_error(&r.labels[LABEL_ERROR_JSON])?)
    } else {
        None
    };

    let metadata = if r.labels[LABEL_METADATA_JSON].is_empty() {
        None
    } else {
        Some(
            serde_json::from_str::<JsonValue>(&r.labels[LABEL_METADATA_JSON])
                .map_err(|e| MappingError::InvalidMetadata(e.to_string()))?,
        )
    };

    let trace_id = if r.labels[LABEL_TRACE_ID].is_empty() {
        None
    } else {
        Some(hex_to_trace_id(&r.labels[LABEL_TRACE_ID])?)
    };

    let client_ip = if r.labels[LABEL_CLIENT_IP].is_empty() {
        None
    } else {
        Some(
            IpAddr::from_str(&r.labels[LABEL_CLIENT_IP])
                .map_err(|_| MappingError::InvalidIp(r.labels[LABEL_CLIENT_IP].clone()))?,
        )
    };

    let method = if r.labels[LABEL_METHOD].is_empty() {
        Method::POST
    } else {
        Method::from_bytes(r.labels[LABEL_METHOD].as_bytes())
            .map_err(|_| MappingError::InvalidMethod(r.labels[LABEL_METHOD].clone()))?
    };

    let http_status = if has_http_status { Some(r.status) } else { None };
    let ttft_ms = if has_ttft { Some(r.latency_detail_ms) } else { None };
    let time_to_close_ms = if has_close_ms { Some(r.counters[COUNTER_CLOSE_MS]) } else { None };

    let usage = Usage {
        input_tokens: r.counters[COUNTER_INPUT_TOKENS],
        output_tokens: r.counters[COUNTER_OUTPUT_TOKENS],
        cache_read_input_tokens: r.counters[COUNTER_CACHE_READ],
        cache_creation_input_tokens: r.counters[COUNTER_CACHE_CREATION],
        reasoning_tokens: r.counters[COUNTER_REASONING],
        audio_input_tokens: r.counters[COUNTER_AUDIO_IN],
        audio_output_tokens: r.counters[COUNTER_AUDIO_OUT],
        image_tokens: r.counters[COUNTER_IMAGE],
        video_seconds: r.counters[COUNTER_VIDEO_SECONDS],
        tool_use_tokens: r.counters[COUNTER_TOOL_USE],
        search_queries: r.counters[COUNTER_SEARCH_QUERIES],
    };

    let latency = Latencies { ttft_ms, total_ms: r.latency_ms, time_to_close_ms };

    Ok(LlmEvent {
        id,
        ts_ns: r.ts_ns,
        user_id: opt_id::<UserId>(&r.dims[DIM_USER_ID]),
        api_key_id: opt_id::<ApiKeyId>(&r.dims[DIM_API_KEY_ID]),
        org_id: opt_id::<OrgId>(&r.dims[DIM_ORG_ID]),
        project_id: opt_id::<ProjectId>(&r.dims[DIM_PROJECT_ID]),
        route_id: RouteId::from(r.dims[DIM_ROUTE_ID].as_str()),
        provider,
        model: SmolStr::new(&r.dims[DIM_MODEL]),
        model_family: opt_smol(&r.dims[DIM_MODEL_FAMILY]),
        endpoint: SmolStr::new(&r.dims[DIM_ENDPOINT]),
        method,
        http_status,
        usage,
        cost_nanodollars: r.metric,
        latency,
        flags: unpack_event_flags(flags_u16),
        error,
        // `request_sha256` / `response_sha256` were removed from the
        // wire in this cutover; keep zero for struct compatibility.
        request_sha256: [0; 32],
        response_sha256: [0; 32],
        client_ip,
        user_agent: opt_smol(&r.labels[LABEL_USER_AGENT]),
        request_id: opt_smol(&r.labels[LABEL_REQUEST_ID]),
        trace_id,
        source: opt_smol(&r.dims[DIM_SOURCE]),
        // `ingested_at` is no longer persisted — callers that need a
        // receive timestamp should inspect the ULID's time component.
        ingested_at: r.ts_ns,
        metadata,
        tier: SmolStr::new(&r.dims[DIM_TIER]),
    })
}

/// Convert the server's [`EventFilter`] into the KeplorDB query shape.
///
/// Status codes map onto `status_min`/`status_max` (exclusive upper
/// bound per KeplorDB convention).  Tag filters now hit dims directly
/// instead of a JSON-path scan.
#[must_use]
pub fn to_query_filter(f: &EventFilter) -> keplordb::QueryFilter<D> {
    let mut dims: [Option<String>; D] = std::array::from_fn(|_| None);
    dims[DIM_USER_ID] = f.user_id.as_ref().map(|s| s.to_string());
    dims[DIM_API_KEY_ID] = f.api_key_id.as_ref().map(|s| s.to_string());
    dims[DIM_MODEL] = f.model.as_ref().map(|s| s.to_string());
    dims[DIM_PROVIDER] = f.provider.as_ref().map(|s| s.to_string());
    dims[DIM_SOURCE] = f.source.as_ref().map(|s| s.to_string());
    dims[DIM_USER_TAG] = f.meta_user_tag.as_ref().map(|s| s.to_string());
    dims[DIM_SESSION_TAG] = f.meta_session_tag.as_ref().map(|s| s.to_string());

    keplordb::QueryFilter {
        dims,
        from_ts_ns: f.from_ts_ns,
        to_ts_ns: f.to_ts_ns,
        status_min: f.http_status_min,
        status_max: f.http_status_max,
        cursor: None,
    }
}

/// Attach a pagination cursor to a filter.
#[must_use]
pub fn with_cursor(
    mut qf: keplordb::QueryFilter<D>,
    cursor: Option<Cursor>,
) -> keplordb::QueryFilter<D> {
    qf.cursor = cursor.map(|c| keplordb::read::query::Cursor(c.0));
    qf
}

// ── Private helpers ───────────────────────────────────────────────────

fn pack_flags(event: &LlmEvent) -> u16 {
    let mut bits: u16 = 0;
    let src = event.flags.bits();
    if src & EventFlags::STREAMING.bits() != 0 {
        bits |= FLAG_STREAMING;
    }
    if src & EventFlags::TOOL_CALLS.bits() != 0 {
        bits |= FLAG_TOOL_CALLS;
    }
    if src & EventFlags::REASONING.bits() != 0 {
        bits |= FLAG_REASONING;
    }
    if src & EventFlags::STREAM_INCOMPLETE.bits() != 0 {
        bits |= FLAG_STREAM_INCOMPLETE;
    }
    if src & EventFlags::CACHED_USED.bits() != 0 {
        bits |= FLAG_CACHED_USED;
    }
    if src & EventFlags::BUDGET_BLOCKED.bits() != 0 {
        bits |= FLAG_BUDGET_BLOCKED;
    }

    if event.latency.ttft_ms.is_some() {
        bits |= FLAG_HAS_TTFT;
    }
    if event.http_status.is_some() {
        bits |= FLAG_HAS_HTTP_STATUS;
    }
    if event.error.is_some() {
        bits |= FLAG_HAS_ERROR;
    }
    if event.latency.time_to_close_ms.is_some() {
        bits |= FLAG_HAS_CLOSE_MS;
    }
    bits
}

fn unpack_event_flags(bits: u16) -> EventFlags {
    let mut out = EventFlags::empty();
    if bits & FLAG_STREAMING != 0 {
        out |= EventFlags::STREAMING;
    }
    if bits & FLAG_TOOL_CALLS != 0 {
        out |= EventFlags::TOOL_CALLS;
    }
    if bits & FLAG_REASONING != 0 {
        out |= EventFlags::REASONING;
    }
    if bits & FLAG_STREAM_INCOMPLETE != 0 {
        out |= EventFlags::STREAM_INCOMPLETE;
    }
    if bits & FLAG_CACHED_USED != 0 {
        out |= EventFlags::CACHED_USED;
    }
    if bits & FLAG_BUDGET_BLOCKED != 0 {
        out |= EventFlags::BUDGET_BLOCKED;
    }
    out
}

fn read_provider(id_key: &str, variant_label: &str) -> Provider {
    // `Provider::from_id_key` falls through to
    // `OpenAICompatible { base_url: <input> }` for unknown strings —
    // which would mis-store the literal `"openai_compatible"` as the
    // base URL.  Special-case it.
    match id_key {
        "openai_compatible" => Provider::OpenAICompatible { base_url: Arc::from(variant_label) },
        other => Provider::from_id_key(other),
    }
}

fn provider_error_type_key(e: &ProviderError) -> &'static str {
    match e {
        ProviderError::RateLimited { .. } => "rate_limited",
        ProviderError::InvalidRequest(_) => "invalid_request",
        ProviderError::AuthFailed => "auth_failed",
        ProviderError::ContextLengthExceeded { .. } => "context_length_exceeded",
        ProviderError::ContentFiltered { .. } => "content_filtered",
        ProviderError::UpstreamTimeout => "upstream_timeout",
        ProviderError::UpstreamUnavailable => "upstream_unavailable",
        ProviderError::Other { .. } => "other",
    }
}

fn trace_id_to_hex(id: &TraceId) -> String {
    let mut out = String::with_capacity(32);
    for b in id.0.iter() {
        use std::fmt::Write;
        let _ = write!(out, "{b:02x}");
    }
    out
}

fn hex_to_trace_id(s: &str) -> Result<TraceId, MappingError> {
    if s.len() != 32 {
        return Err(MappingError::InvalidTraceId(s.to_owned()));
    }
    let mut bytes = [0u8; 16];
    for i in 0..16 {
        let hex = &s[i * 2..i * 2 + 2];
        bytes[i] =
            u8::from_str_radix(hex, 16).map_err(|_| MappingError::InvalidTraceId(s.to_owned()))?;
    }
    Ok(TraceId(bytes))
}

fn extract_tags(metadata: Option<&JsonValue>) -> (String, String) {
    let Some(JsonValue::Object(map)) = metadata else {
        return (String::new(), String::new());
    };
    let user_tag = map.get("user_tag").and_then(JsonValue::as_str).unwrap_or("").to_owned();
    let session_tag = map.get("session_tag").and_then(JsonValue::as_str).unwrap_or("").to_owned();
    (user_tag, session_tag)
}

fn opt_smol(s: &str) -> Option<SmolStr> {
    if s.is_empty() {
        None
    } else {
        Some(SmolStr::new(s))
    }
}

fn opt_id<T: for<'a> From<&'a str>>(s: &str) -> Option<T> {
    if s.is_empty() {
        None
    } else {
        Some(T::from(s))
    }
}

/// Encode a [`ProviderError`] into a compact, round-trip-safe JSON
/// string.
///
/// The canonical `#[serde(tag = "kind")]` encoding on [`ProviderError`]
/// cannot serialise tuple variants like `InvalidRequest(String)` into a
/// map, so we use our own flat schema keyed by `t` (type) plus
/// variant-specific fields (`m` for message, `r` for retry seconds,
/// `l` for context limit, `s` for status).
fn encode_error(e: &ProviderError) -> String {
    match e {
        ProviderError::RateLimited { retry_after } => match retry_after {
            Some(d) => format!(r#"{{"t":"rate_limited","r":{}}}"#, d.as_secs()),
            None => r#"{"t":"rate_limited"}"#.to_owned(),
        },
        ProviderError::InvalidRequest(msg) => {
            let m = serde_json::to_string(msg.as_str()).unwrap_or_else(|_| "\"\"".to_owned());
            format!(r#"{{"t":"invalid_request","m":{m}}}"#)
        },
        ProviderError::AuthFailed => r#"{"t":"auth_failed"}"#.to_owned(),
        ProviderError::ContextLengthExceeded { limit } => {
            format!(r#"{{"t":"context_length_exceeded","l":{limit}}}"#)
        },
        ProviderError::ContentFiltered { reason } => {
            let m = serde_json::to_string(reason.as_str()).unwrap_or_else(|_| "\"\"".to_owned());
            format!(r#"{{"t":"content_filtered","m":{m}}}"#)
        },
        ProviderError::UpstreamTimeout => r#"{"t":"upstream_timeout"}"#.to_owned(),
        ProviderError::UpstreamUnavailable => r#"{"t":"upstream_unavailable"}"#.to_owned(),
        ProviderError::Other { status, message } => {
            let m = serde_json::to_string(message.as_str()).unwrap_or_else(|_| "\"\"".to_owned());
            format!(r#"{{"t":"other","s":{status},"m":{m}}}"#)
        },
    }
}

fn decode_error(s: &str) -> Result<ProviderError, MappingError> {
    let v: JsonValue =
        serde_json::from_str(s).map_err(|e| MappingError::InvalidError(e.to_string()))?;
    let obj = v.as_object().ok_or_else(|| MappingError::InvalidError("not an object".into()))?;
    let t = obj
        .get("t")
        .and_then(JsonValue::as_str)
        .ok_or_else(|| MappingError::InvalidError("missing discriminator".into()))?;
    let msg = obj.get("m").and_then(JsonValue::as_str);
    match t {
        "rate_limited" => {
            let retry_after = obj.get("r").and_then(JsonValue::as_u64).map(Duration::from_secs);
            Ok(ProviderError::RateLimited { retry_after })
        },
        "invalid_request" => Ok(ProviderError::InvalidRequest(msg.unwrap_or("").to_owned())),
        "auth_failed" => Ok(ProviderError::AuthFailed),
        "context_length_exceeded" => {
            let limit = obj.get("l").and_then(JsonValue::as_u64).unwrap_or(0) as u32;
            Ok(ProviderError::ContextLengthExceeded { limit })
        },
        "content_filtered" => {
            Ok(ProviderError::ContentFiltered { reason: SmolStr::new(msg.unwrap_or("")) })
        },
        "upstream_timeout" => Ok(ProviderError::UpstreamTimeout),
        "upstream_unavailable" => Ok(ProviderError::UpstreamUnavailable),
        "other" => {
            let status = obj.get("s").and_then(JsonValue::as_u64).unwrap_or(0) as u16;
            Ok(ProviderError::Other { status, message: SmolStr::new(msg.unwrap_or("")) })
        },
        _ => Err(MappingError::InvalidError(format!("unknown discriminator: {t}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_event() -> LlmEvent {
        LlmEvent {
            id: EventId::new(),
            ts_ns: 1_700_000_000_000_000_000,
            user_id: Some(UserId::from("alice")),
            api_key_id: Some(ApiKeyId::from("key_123")),
            org_id: Some(OrgId::from("org_acme")),
            project_id: Some(ProjectId::from("proj_x")),
            route_id: RouteId::from("chat"),
            provider: Provider::OpenAI,
            model: SmolStr::new("gpt-4o"),
            model_family: Some(SmolStr::new("gpt-4")),
            endpoint: SmolStr::new("/v1/chat/completions"),
            method: Method::POST,
            http_status: Some(200),
            usage: Usage {
                input_tokens: 1_200,
                output_tokens: 850,
                cache_read_input_tokens: 100,
                cache_creation_input_tokens: 200,
                reasoning_tokens: 300,
                audio_input_tokens: 0,
                audio_output_tokens: 0,
                image_tokens: 50,
                video_seconds: 0,
                tool_use_tokens: 25,
                search_queries: 1,
            },
            cost_nanodollars: 5_123_000,
            latency: Latencies { ttft_ms: Some(120), total_ms: 1_500, time_to_close_ms: Some(10) },
            flags: EventFlags::STREAMING | EventFlags::REASONING,
            error: None,
            request_sha256: [0; 32],
            response_sha256: [0; 32],
            client_ip: Some("192.0.2.1".parse().unwrap()),
            user_agent: Some(SmolStr::new("curl/8.0")),
            request_id: Some(SmolStr::new("req_abc")),
            trace_id: Some(TraceId([
                0x4b, 0xf9, 0x2f, 0x35, 0x77, 0xb3, 0x4d, 0xa6, 0xa3, 0xce, 0x92, 0x9d, 0x0e, 0x0e,
                0x47, 0x36,
            ])),
            source: Some(SmolStr::new("litellm")),
            ingested_at: 1_700_000_000_000_000_100,
            metadata: Some(serde_json::json!({
                "user_tag": "demo",
                "session_tag": "s_xyz",
                "extra": 42,
            })),
            tier: SmolStr::new("pro"),
        }
    }

    fn event_ref_from(le: &LogEvent<D, C, L>) -> EventRef<D, C, L> {
        EventRef {
            id: le.id.clone(),
            ts_ns: le.ts_ns,
            metric: le.metric,
            counters: le.counters,
            latency_ms: le.latency_ms,
            latency_detail_ms: le.latency_detail_ms,
            status: le.status,
            flags: le.flags,
            dims: le.dims.clone(),
            labels: le.labels.clone(),
        }
    }

    #[test]
    fn round_trip_preserves_all_fields() {
        let ev = sample_event();
        let log = to_log_event(&ev);
        let back = from_event_ref(&event_ref_from(&log)).unwrap();

        assert_eq!(back.id, ev.id);
        assert_eq!(back.ts_ns, ev.ts_ns);
        assert_eq!(back.user_id, ev.user_id);
        assert_eq!(back.api_key_id, ev.api_key_id);
        assert_eq!(back.org_id, ev.org_id);
        assert_eq!(back.project_id, ev.project_id);
        assert_eq!(back.route_id, ev.route_id);
        assert_eq!(back.provider, ev.provider);
        assert_eq!(back.model, ev.model);
        assert_eq!(back.model_family, ev.model_family);
        assert_eq!(back.endpoint, ev.endpoint);
        assert_eq!(back.method, ev.method);
        assert_eq!(back.http_status, ev.http_status);
        assert_eq!(back.usage, ev.usage);
        assert_eq!(back.cost_nanodollars, ev.cost_nanodollars);
        assert_eq!(back.latency, ev.latency);
        assert_eq!(back.flags, ev.flags);
        assert_eq!(back.error, ev.error);
        assert_eq!(back.client_ip, ev.client_ip);
        assert_eq!(back.user_agent, ev.user_agent);
        assert_eq!(back.request_id, ev.request_id);
        assert_eq!(back.trace_id, ev.trace_id);
        assert_eq!(back.source, ev.source);
        assert_eq!(back.metadata, ev.metadata);
        assert_eq!(back.tier, ev.tier);
    }

    #[test]
    fn round_trip_empty_optionals() {
        let mut ev = sample_event();
        ev.user_id = None;
        ev.api_key_id = None;
        ev.org_id = None;
        ev.project_id = None;
        ev.model_family = None;
        ev.http_status = None;
        ev.latency.ttft_ms = None;
        ev.latency.time_to_close_ms = None;
        ev.client_ip = None;
        ev.user_agent = None;
        ev.request_id = None;
        ev.trace_id = None;
        ev.source = None;
        ev.metadata = None;

        let log = to_log_event(&ev);
        let back = from_event_ref(&event_ref_from(&log)).unwrap();

        assert_eq!(back.user_id, None);
        assert_eq!(back.api_key_id, None);
        assert_eq!(back.org_id, None);
        assert_eq!(back.project_id, None);
        assert_eq!(back.model_family, None);
        assert_eq!(back.http_status, None);
        assert_eq!(back.latency.ttft_ms, None);
        assert_eq!(back.latency.time_to_close_ms, None);
        assert_eq!(back.client_ip, None);
        assert_eq!(back.user_agent, None);
        assert_eq!(back.request_id, None);
        assert_eq!(back.trace_id, None);
        assert_eq!(back.source, None);
        assert_eq!(back.metadata, None);
    }

    #[test]
    fn ttft_some_zero_distinguished_from_none() {
        let mut ev = sample_event();
        ev.latency.ttft_ms = Some(0);
        let log = to_log_event(&ev);
        let back = from_event_ref(&event_ref_from(&log)).unwrap();
        assert_eq!(back.latency.ttft_ms, Some(0));

        ev.latency.ttft_ms = None;
        let log = to_log_event(&ev);
        let back = from_event_ref(&event_ref_from(&log)).unwrap();
        assert_eq!(back.latency.ttft_ms, None);
    }

    #[test]
    fn http_status_some_zero_distinguished_from_none() {
        let mut ev = sample_event();
        ev.http_status = Some(0);
        let log = to_log_event(&ev);
        let back = from_event_ref(&event_ref_from(&log)).unwrap();
        assert_eq!(back.http_status, Some(0));

        ev.http_status = None;
        let log = to_log_event(&ev);
        let back = from_event_ref(&event_ref_from(&log)).unwrap();
        assert_eq!(back.http_status, None);
    }

    #[test]
    fn close_ms_some_zero_distinguished_from_none() {
        let mut ev = sample_event();
        ev.latency.time_to_close_ms = Some(0);
        let log = to_log_event(&ev);
        let back = from_event_ref(&event_ref_from(&log)).unwrap();
        assert_eq!(back.latency.time_to_close_ms, Some(0));
    }

    #[test]
    fn openai_compatible_provider_roundtrips() {
        let mut ev = sample_event();
        ev.provider =
            Provider::OpenAICompatible { base_url: Arc::from("https://custom.example.com") };

        let log = to_log_event(&ev);
        assert_eq!(log.dims[DIM_PROVIDER], "openai_compatible");
        assert_eq!(log.labels[LABEL_PROVIDER_VARIANT], "https://custom.example.com");

        let back = from_event_ref(&event_ref_from(&log)).unwrap();
        assert_eq!(back.provider, ev.provider);
    }

    #[test]
    fn provider_variants_all_roundtrip() {
        let variants = [
            Provider::OpenAI,
            Provider::Anthropic,
            Provider::Gemini,
            Provider::GeminiVertex,
            Provider::Bedrock,
            Provider::AzureOpenAI,
            Provider::Mistral,
            Provider::Groq,
            Provider::XAi,
            Provider::DeepSeek,
            Provider::Cohere,
            Provider::OpenRouter,
            Provider::Ollama,
        ];
        for p in variants {
            let mut ev = sample_event();
            ev.provider = p.clone();
            let log = to_log_event(&ev);
            let back = from_event_ref(&event_ref_from(&log)).unwrap();
            assert_eq!(back.provider, p);
        }
    }

    #[test]
    fn provider_error_variants_roundtrip() {
        use std::time::Duration;
        let errors = [
            ProviderError::RateLimited { retry_after: Some(Duration::from_secs(30)) },
            ProviderError::RateLimited { retry_after: None },
            ProviderError::InvalidRequest("bad input".into()),
            ProviderError::AuthFailed,
            ProviderError::ContextLengthExceeded { limit: 128_000 },
            ProviderError::ContentFiltered { reason: SmolStr::new("hate_speech") },
            ProviderError::UpstreamTimeout,
            ProviderError::UpstreamUnavailable,
            ProviderError::Other { status: 418, message: SmolStr::new("im a teapot") },
        ];
        for e in errors {
            let mut ev = sample_event();
            ev.error = Some(e.clone());
            let log = to_log_event(&ev);
            assert_eq!(log.counters[COUNTER_IS_ERROR], 1);
            let back = from_event_ref(&event_ref_from(&log)).unwrap();
            assert_eq!(back.error, Some(e));
        }
    }

    #[test]
    fn tags_promoted_to_dims() {
        let ev = sample_event();
        let log = to_log_event(&ev);
        assert_eq!(log.dims[DIM_USER_TAG], "demo");
        assert_eq!(log.dims[DIM_SESSION_TAG], "s_xyz");
    }

    #[test]
    fn metadata_without_tags_leaves_dims_empty() {
        let mut ev = sample_event();
        ev.metadata = Some(serde_json::json!({ "something_else": true }));
        let log = to_log_event(&ev);
        assert_eq!(log.dims[DIM_USER_TAG], "");
        assert_eq!(log.dims[DIM_SESSION_TAG], "");
    }

    #[test]
    fn is_error_counter_set_only_on_errors() {
        let mut ev = sample_event();
        ev.error = None;
        let log = to_log_event(&ev);
        assert_eq!(log.counters[COUNTER_IS_ERROR], 0);

        ev.error = Some(ProviderError::AuthFailed);
        let log = to_log_event(&ev);
        assert_eq!(log.counters[COUNTER_IS_ERROR], 1);
    }

    #[test]
    fn trace_id_hex_roundtrip() {
        let id = TraceId([
            0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0x00, 0xff, 0x00, 0xff, 0xde, 0xad,
            0xbe, 0xef,
        ]);
        let hex = trace_id_to_hex(&id);
        assert_eq!(hex, "0123456789abcdef00ff00ffdeadbeef");
        let back = hex_to_trace_id(&hex).unwrap();
        assert_eq!(back, id);
    }

    #[test]
    fn trace_id_rejects_wrong_length() {
        assert!(hex_to_trace_id("abcd").is_err());
        assert!(hex_to_trace_id(&"a".repeat(33)).is_err());
    }

    #[test]
    fn trace_id_rejects_non_hex() {
        assert!(hex_to_trace_id("zz23456789abcdef00ff00ffdeadbeef").is_err());
    }

    #[test]
    fn filter_maps_all_fields() {
        let f = EventFilter {
            user_id: Some("alice".into()),
            api_key_id: Some("key1".into()),
            model: Some("gpt-4o".into()),
            provider: Some("openai".into()),
            source: Some("litellm".into()),
            from_ts_ns: Some(1_000),
            to_ts_ns: Some(2_000),
            http_status_min: Some(400),
            http_status_max: Some(500),
            meta_user_tag: Some("demo".into()),
            meta_session_tag: Some("s_xyz".into()),
        };
        let qf = to_query_filter(&f);
        assert_eq!(qf.dims[DIM_USER_ID].as_deref(), Some("alice"));
        assert_eq!(qf.dims[DIM_API_KEY_ID].as_deref(), Some("key1"));
        assert_eq!(qf.dims[DIM_MODEL].as_deref(), Some("gpt-4o"));
        assert_eq!(qf.dims[DIM_PROVIDER].as_deref(), Some("openai"));
        assert_eq!(qf.dims[DIM_SOURCE].as_deref(), Some("litellm"));
        assert_eq!(qf.dims[DIM_USER_TAG].as_deref(), Some("demo"));
        assert_eq!(qf.dims[DIM_SESSION_TAG].as_deref(), Some("s_xyz"));
        assert_eq!(qf.from_ts_ns, Some(1_000));
        assert_eq!(qf.to_ts_ns, Some(2_000));
        assert_eq!(qf.status_min, Some(400));
        assert_eq!(qf.status_max, Some(500));
    }

    #[test]
    fn empty_filter_produces_all_nones() {
        let qf = to_query_filter(&EventFilter::default());
        for d in &qf.dims {
            assert!(d.is_none());
        }
        assert!(qf.from_ts_ns.is_none());
        assert!(qf.status_min.is_none());
    }

    #[test]
    fn flag_bits_distinct() {
        // Presence bits must not collide with existing flag bits.
        let domain_flags = FLAG_STREAMING
            | FLAG_TOOL_CALLS
            | FLAG_REASONING
            | FLAG_STREAM_INCOMPLETE
            | FLAG_CACHED_USED
            | FLAG_BUDGET_BLOCKED;
        let presence_flags =
            FLAG_HAS_TTFT | FLAG_HAS_HTTP_STATUS | FLAG_HAS_ERROR | FLAG_HAS_CLOSE_MS;
        assert_eq!(domain_flags & presence_flags, 0);
    }

    // Guard against someone silently bumping a const past the cap. These
    // are compile-time checks; no runtime test needed.
    const _: () = assert!(D <= 256);
    const _: () = assert!(C <= 64);
    const _: () = assert!(L <= 64);
}
