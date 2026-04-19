//! [`StoredEvent`] — a fully serializable representation of [`LlmEvent`]
//! for JSONL archival to S3/R2.
//!
//! [`LlmEvent`] deliberately does not derive `Serialize` because
//! `http::Method` has no stable serde impl.  `StoredEvent` maps method
//! to a string and skips vestigial SHA fields, preserving all 40+
//! meaningful fields so that archived data is lossless.

use serde::{Deserialize, Serialize};

use keplor_core::{
    EventFlags, EventId, Latencies, LlmEvent, Provider, ProviderError, TraceId, Usage,
};

/// Fully serializable event for JSONL archive files.
///
/// Every meaningful field from [`LlmEvent`] is included.  The vestigial
/// `request_sha256` / `response_sha256` columns are omitted.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredEvent {
    /// Primary key — ULID string.
    pub id: String,
    /// Wall-clock capture time in nanoseconds since Unix epoch.
    pub ts_ns: i64,
    /// Caller-provided user id.
    pub user_id: Option<String>,
    /// API key id.
    pub api_key_id: Option<String>,
    /// Organisation id.
    pub org_id: Option<String>,
    /// Project id.
    pub project_id: Option<String>,
    /// Logical route name.
    pub route_id: String,
    /// Provider enum.
    pub provider: Provider,
    /// Model name.
    pub model: String,
    /// Model family.
    pub model_family: Option<String>,
    /// Request endpoint path.
    pub endpoint: String,
    /// HTTP method as string (e.g. `"POST"`).
    pub method: String,
    /// HTTP status code.
    pub http_status: Option<u16>,
    /// Token usage counters.
    pub usage: Usage,
    /// Cost in nanodollars.
    pub cost_nanodollars: i64,
    /// Latency breakdown.
    pub latency: Latencies,
    /// Per-event boolean flags.
    pub flags: EventFlags,
    /// Upstream error, if any.
    pub error: Option<ProviderError>,
    /// Client source IP.
    pub client_ip: Option<String>,
    /// Client user-agent.
    pub user_agent: Option<String>,
    /// Provider-returned request id.
    pub request_id: Option<String>,
    /// W3C trace-context trace id.
    pub trace_id: Option<TraceId>,
    /// Ingestion source (e.g. `"obol"`, `"litellm"`).
    pub source: Option<String>,
    /// Server-side ingestion timestamp (nanoseconds).
    pub ingested_at: i64,
    /// Arbitrary metadata JSON.
    pub metadata: Option<serde_json::Value>,
    /// Retention tier.
    pub tier: String,
}

impl From<&LlmEvent> for StoredEvent {
    fn from(e: &LlmEvent) -> Self {
        Self {
            id: e.id.to_string(),
            ts_ns: e.ts_ns,
            user_id: e.user_id.as_ref().map(|s| s.to_string()),
            api_key_id: e.api_key_id.as_ref().map(|s| s.to_string()),
            org_id: e.org_id.as_ref().map(|s| s.to_string()),
            project_id: e.project_id.as_ref().map(|s| s.to_string()),
            route_id: e.route_id.to_string(),
            provider: e.provider.clone(),
            model: e.model.to_string(),
            model_family: e.model_family.as_ref().map(|s| s.to_string()),
            endpoint: e.endpoint.to_string(),
            method: e.method.as_str().to_owned(),
            http_status: e.http_status,
            usage: e.usage,
            cost_nanodollars: e.cost_nanodollars,
            latency: e.latency,
            flags: e.flags,
            error: e.error.clone(),
            client_ip: e.client_ip.map(|ip| ip.to_string()),
            user_agent: e.user_agent.as_ref().map(|s| s.to_string()),
            request_id: e.request_id.as_ref().map(|s| s.to_string()),
            trace_id: e.trace_id,
            source: e.source.as_ref().map(|s| s.to_string()),
            ingested_at: e.ingested_at,
            metadata: e.metadata.clone(),
            tier: e.tier.to_string(),
        }
    }
}

impl TryFrom<StoredEvent> for LlmEvent {
    type Error = crate::error::StoreError;

    fn try_from(s: StoredEvent) -> Result<Self, Self::Error> {
        let id: EventId = s.id.parse().map_err(|_| {
            crate::error::StoreError::Internal(format!("invalid event id: {}", s.id))
        })?;
        let method = http::Method::from_bytes(s.method.as_bytes()).unwrap_or(http::Method::POST);

        Ok(Self {
            id,
            ts_ns: s.ts_ns,
            user_id: s.user_id.map(|v| v.as_str().into()),
            api_key_id: s.api_key_id.map(|v| v.as_str().into()),
            org_id: s.org_id.map(|v| v.as_str().into()),
            project_id: s.project_id.map(|v| v.as_str().into()),
            route_id: s.route_id.as_str().into(),
            provider: s.provider,
            model: smol_str::SmolStr::new(&s.model),
            model_family: s.model_family.map(|v| smol_str::SmolStr::new(&v)),
            endpoint: smol_str::SmolStr::new(&s.endpoint),
            method,
            http_status: s.http_status,
            usage: s.usage,
            cost_nanodollars: s.cost_nanodollars,
            latency: s.latency,
            flags: s.flags,
            error: s.error,
            request_sha256: [0u8; 32],
            response_sha256: [0u8; 32],
            client_ip: s.client_ip.and_then(|v| v.parse().ok()),
            user_agent: s.user_agent.map(|v| smol_str::SmolStr::new(&v)),
            request_id: s.request_id.map(|v| smol_str::SmolStr::new(&v)),
            trace_id: s.trace_id,
            source: s.source.map(|v| smol_str::SmolStr::new(&v)),
            ingested_at: s.ingested_at,
            metadata: s.metadata,
            tier: smol_str::SmolStr::new(&s.tier),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use keplor_core::*;
    use smol_str::SmolStr;

    fn test_event() -> LlmEvent {
        LlmEvent {
            id: EventId::new(),
            ts_ns: 1_700_000_000_000_000_000,
            user_id: Some(UserId::from("alice")),
            api_key_id: Some(ApiKeyId::from("key_1")),
            org_id: Some(OrgId::from("org_1")),
            project_id: Some(ProjectId::from("proj_1")),
            route_id: RouteId::from("chat"),
            provider: Provider::OpenAI,
            model: SmolStr::new("gpt-4o"),
            model_family: Some(SmolStr::new("gpt-4")),
            endpoint: SmolStr::new("/v1/chat/completions"),
            method: http::Method::POST,
            http_status: Some(200),
            usage: Usage { input_tokens: 100, output_tokens: 50, ..Usage::default() },
            cost_nanodollars: 750_000,
            latency: Latencies { ttft_ms: Some(25), total_ms: 300, time_to_close_ms: None },
            flags: EventFlags::STREAMING | EventFlags::TOOL_CALLS,
            error: None,
            request_sha256: [0u8; 32],
            response_sha256: [0u8; 32],
            client_ip: Some("127.0.0.1".parse().unwrap()),
            user_agent: Some(SmolStr::new("test/1.0")),
            request_id: Some(SmolStr::new("req_abc")),
            trace_id: Some("00112233445566778899aabbccddeeff".parse().unwrap()),
            source: Some(SmolStr::new("obol")),
            ingested_at: 1_700_000_001_000_000_000,
            metadata: Some(serde_json::json!({"user_tag": "demo"})),
            tier: SmolStr::new("pro"),
        }
    }

    #[test]
    fn roundtrip_stored_event() {
        let event = test_event();
        let stored = StoredEvent::from(&event);

        // Serialize to JSON and back.
        let json = serde_json::to_string(&stored).unwrap();
        let back: StoredEvent = serde_json::from_str(&json).unwrap();
        let restored: LlmEvent = back.try_into().unwrap();

        assert_eq!(restored.id, event.id);
        assert_eq!(restored.ts_ns, event.ts_ns);
        assert_eq!(restored.user_id.as_ref().unwrap().as_str(), "alice");
        assert_eq!(restored.provider, Provider::OpenAI);
        assert_eq!(restored.model, "gpt-4o");
        assert_eq!(restored.usage.input_tokens, 100);
        assert_eq!(restored.cost_nanodollars, 750_000);
        assert_eq!(restored.flags, EventFlags::STREAMING | EventFlags::TOOL_CALLS);
        assert_eq!(restored.trace_id, event.trace_id);
        assert_eq!(restored.source.as_deref(), Some("obol"));
        assert_eq!(restored.tier, "pro");
    }

    #[test]
    fn stored_event_jsonl_line() {
        let event = test_event();
        let stored = StoredEvent::from(&event);
        let json = serde_json::to_string(&stored).unwrap();
        // JSONL: no newlines in the serialized output.
        assert!(!json.contains('\n'));
        // Deserializes cleanly.
        let _: StoredEvent = serde_json::from_str(&json).unwrap();
    }
}
