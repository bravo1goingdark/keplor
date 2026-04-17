//! Ingestion JSON schema — the wire format external proxies POST.

use serde::{Deserialize, Serialize};

/// The JSON body for `POST /v1/events`.
///
/// Any LLM proxy (LiteLLM, OpenRouter, custom gateways) can POST events
/// in this format.  Keplor computes cost from usage + its pricing catalog —
/// `cost_nanodollars` is intentionally absent from this schema.
#[derive(Debug, Deserialize)]
pub struct IngestEvent {
    // --- required --------------------------------------------------------
    /// Model name as reported by the proxy (e.g. `"gpt-4o"`).
    pub model: String,

    /// Provider identifier: `"openai"`, `"anthropic"`, `"gemini"`, etc.
    pub provider: String,

    // --- optional (all have sensible defaults) ---------------------------
    /// Token usage counters.
    #[serde(default)]
    pub usage: IngestUsage,

    /// Latency breakdown.
    #[serde(default)]
    pub latency: IngestLatency,

    /// Event timestamp — ISO 8601 string or epoch nanoseconds.
    /// Defaults to server wall-clock time if absent.
    pub timestamp: Option<TimestampInput>,

    /// HTTP method used for the upstream call (default `"POST"`).
    #[serde(default = "default_method")]
    pub method: String,

    /// API endpoint path (e.g. `"/v1/chat/completions"`).
    #[serde(default)]
    pub endpoint: String,

    /// HTTP status code returned by the upstream.
    pub http_status: Option<u16>,

    /// Name of the proxy/system sending this event.
    pub source: Option<String>,

    // --- attribution -----------------------------------------------------
    /// Caller-provided user id.
    pub user_id: Option<String>,
    /// Stable id for the API key used on the request.
    pub api_key_id: Option<String>,
    /// Organisation id for cost rollups.
    pub org_id: Option<String>,
    /// Project id under the organisation.
    pub project_id: Option<String>,
    /// Logical route name (e.g. `"chat"`, `"embeddings"`).
    pub route_id: Option<String>,

    // --- flags -----------------------------------------------------------
    /// Per-event boolean signals.
    #[serde(default)]
    pub flags: IngestFlags,

    // --- error -----------------------------------------------------------
    /// Upstream error, if any.
    pub error: Option<IngestError>,

    // --- observability ---------------------------------------------------
    /// W3C trace-context trace id.
    pub trace_id: Option<String>,
    /// Provider-returned request id.
    pub request_id: Option<String>,
    /// Client source IP.
    pub client_ip: Option<String>,
    /// Client user-agent string.
    pub user_agent: Option<String>,

    // --- bodies (optional, stored compressed) ----------------------------
    /// Raw request body as a JSON value.
    pub request_body: Option<serde_json::Value>,
    /// Raw response body as a JSON value.
    pub response_body: Option<serde_json::Value>,

    // --- extensibility ---------------------------------------------------
    /// Arbitrary metadata — stored but not indexed in v1.
    pub metadata: Option<serde_json::Value>,
}

/// Token usage counters — mirrors [`keplor_core::Usage`] 1:1.
#[derive(Debug, Default, Deserialize)]
pub struct IngestUsage {
    #[serde(default)]
    pub input_tokens: u32,
    #[serde(default)]
    pub output_tokens: u32,
    #[serde(default)]
    pub cache_read_input_tokens: u32,
    #[serde(default)]
    pub cache_creation_input_tokens: u32,
    #[serde(default)]
    pub reasoning_tokens: u32,
    #[serde(default)]
    pub audio_input_tokens: u32,
    #[serde(default)]
    pub audio_output_tokens: u32,
    #[serde(default)]
    pub image_tokens: u32,
    #[serde(default)]
    pub tool_use_tokens: u32,
}

/// Latency breakdown.
#[derive(Debug, Default, Deserialize)]
pub struct IngestLatency {
    /// Time to first byte in milliseconds.
    pub ttft_ms: Option<u32>,
    /// End-to-end latency in milliseconds.
    #[serde(default)]
    pub total_ms: u32,
    /// Time the server spent keeping the stream open after the last chunk.
    pub time_to_close_ms: Option<u32>,
}

/// Per-event boolean signals.
#[derive(Debug, Default, Deserialize)]
pub struct IngestFlags {
    #[serde(default)]
    pub streaming: bool,
    #[serde(default)]
    pub tool_calls: bool,
    #[serde(default)]
    pub reasoning: bool,
    #[serde(default)]
    pub stream_incomplete: bool,
    #[serde(default)]
    pub cache_used: bool,
}

/// Upstream error.
#[derive(Debug, Deserialize)]
pub struct IngestError {
    /// Error kind: `"rate_limited"`, `"auth_failed"`, etc.
    pub kind: String,
    /// Human-readable message.
    pub message: Option<String>,
    /// HTTP status of the error response.
    pub status: Option<u16>,
}

/// Accepts both ISO 8601 strings and epoch nanoseconds.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum TimestampInput {
    /// Epoch nanoseconds.
    EpochNanos(i64),
    /// ISO 8601 date-time string.
    Iso8601(String),
}

/// Batch request wrapper.
#[derive(Debug, Deserialize)]
pub struct BatchRequest {
    /// Array of events to ingest.
    pub events: Vec<IngestEvent>,
}

/// Response returned from single-event ingestion.
#[derive(Debug, Serialize)]
pub struct IngestResponse {
    /// Event id (ULID string).
    pub id: String,
    /// Computed cost in nanodollars.
    pub cost_nanodollars: i64,
    /// Normalised model name.
    pub model: String,
    /// Normalised provider key.
    pub provider: String,
}

/// Response returned from batch ingestion.
#[derive(Debug, Serialize)]
pub struct BatchResponse {
    /// Per-event results.
    pub results: Vec<BatchItemResult>,
    /// Number of events successfully accepted.
    pub accepted: usize,
    /// Number of events rejected.
    pub rejected: usize,
}

/// Result for a single item in a batch.
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum BatchItemResult {
    /// Successfully ingested.
    Ok(IngestResponse),
    /// Rejected with an error message.
    Err {
        /// Error description.
        error: String,
    },
}

fn default_method() -> String {
    "POST".to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimal_event_deserializes() {
        let json = r#"{"model":"gpt-4o","provider":"openai"}"#;
        let event: IngestEvent = serde_json::from_str(json).unwrap();
        assert_eq!(event.model, "gpt-4o");
        assert_eq!(event.provider, "openai");
        assert_eq!(event.method, "POST");
        assert_eq!(event.usage.input_tokens, 0);
    }

    #[test]
    fn full_event_deserializes() {
        let json = r#"{
            "model": "claude-sonnet-4-20250514",
            "provider": "anthropic",
            "usage": {"input_tokens": 1000, "output_tokens": 500, "cache_read_input_tokens": 200},
            "latency": {"ttft_ms": 25, "total_ms": 300},
            "timestamp": 1700000000000000000,
            "http_status": 200,
            "source": "litellm",
            "user_id": "user_1",
            "flags": {"streaming": true, "cache_used": true},
            "request_body": {"messages": [{"role": "user", "content": "hello"}]},
            "metadata": {"custom_field": "value"}
        }"#;
        let event: IngestEvent = serde_json::from_str(json).unwrap();
        assert_eq!(event.usage.input_tokens, 1000);
        assert_eq!(event.usage.cache_read_input_tokens, 200);
        assert!(event.flags.streaming);
        assert!(event.flags.cache_used);
        assert!(matches!(event.timestamp, Some(TimestampInput::EpochNanos(1700000000000000000))));
    }

    #[test]
    fn iso_timestamp_parses() {
        let json = r#"{"model":"gpt-4o","provider":"openai","timestamp":"2024-01-15T10:30:00Z"}"#;
        let event: IngestEvent = serde_json::from_str(json).unwrap();
        assert!(matches!(event.timestamp, Some(TimestampInput::Iso8601(_))));
    }

    #[test]
    fn batch_request_parses() {
        let json = r#"{"events":[
            {"model":"gpt-4o","provider":"openai"},
            {"model":"claude-sonnet-4-20250514","provider":"anthropic"}
        ]}"#;
        let batch: BatchRequest = serde_json::from_str(json).unwrap();
        assert_eq!(batch.events.len(), 2);
    }
}
