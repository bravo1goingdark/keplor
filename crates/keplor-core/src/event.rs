//! [`LlmEvent`] — the canonical captured-request record, and the small
//! helper types ([`Latencies`], [`TraceId`]) it embeds.
//!
//! `LlmEvent` deliberately does **not** derive `Serialize`/`Deserialize`:
//! `http::Method` has no stable serde impl and defining one here would
//! couple the wire format to an upstream crate version.  Phase 3 adds a
//! dedicated `StoredEvent` wire type that maps method to a `SmolStr`.

use std::fmt;
use std::net::IpAddr;
use std::str::FromStr;

use http::Method;
use serde::{Deserialize, Serialize};
use smol_str::SmolStr;

use crate::{
    ApiKeyId, CoreError, EventFlags, EventId, OrgId, ProjectId, Provider, ProviderError, RouteId,
    Usage, UserId,
};

/// The canonical captured-request record.  One row in `llm_events`.
#[derive(Debug, Clone)]
pub struct LlmEvent {
    /// Primary key — time-sortable ULID.
    pub id: EventId,
    /// Wall-clock capture time in **nanoseconds since the Unix epoch**.
    pub ts_ns: i64,
    /// Caller-provided user id (`Option` because anon traffic is valid).
    pub user_id: Option<UserId>,
    /// Stable id for the API key used on the request.
    pub api_key_id: Option<ApiKeyId>,
    /// Organisation attribution for cost rollups.
    pub org_id: Option<OrgId>,
    /// Project attribution under [`LlmEvent::org_id`].
    pub project_id: Option<ProjectId>,
    /// Logical route name (`"chat"`, `"embeddings"`, …).
    pub route_id: RouteId,
    /// Provider surface the request targeted.
    pub provider: Provider,
    /// Model name as reported by the client (e.g. `"gpt-4o-mini"`).
    pub model: SmolStr,
    /// Family bucket for the model (e.g. `"gpt-4"`, `"claude-3-5"`).
    pub model_family: Option<SmolStr>,
    /// Request path relative to the provider base (e.g.
    /// `"/v1/chat/completions"`).
    pub endpoint: SmolStr,
    /// HTTP method.
    pub method: Method,
    /// HTTP status (`None` if the request never got a response).
    pub http_status: Option<u16>,
    /// Normalised token counters.
    pub usage: Usage,
    /// Computed cost in int64 nanodollars (see [`crate::Cost`]).
    pub cost_nanodollars: i64,
    /// Latency breakdown.
    pub latency: Latencies,
    /// Per-event boolean signals.
    pub flags: EventFlags,
    /// Normalised upstream error, if any.
    pub error: Option<ProviderError>,
    /// SHA-256 of the raw (pre-compression) request bytes.
    pub request_sha256: [u8; 32],
    /// SHA-256 of the raw response bytes.
    pub response_sha256: [u8; 32],
    /// Client source IP (`None` when the proxy is behind a trusted
    /// reverse proxy and we don't trust the `X-Forwarded-For` chain).
    pub client_ip: Option<IpAddr>,
    /// Client-side user-agent string.
    pub user_agent: Option<SmolStr>,
    /// Provider-returned request id (e.g. OpenAI `x-request-id`).
    pub request_id: Option<SmolStr>,
    /// W3C trace context id, if the request carried one.
    pub trace_id: Option<TraceId>,
    /// Identifier for the system that sent this event (e.g. `"litellm"`,
    /// `"openrouter"`, `"custom-gateway"`).
    pub source: Option<SmolStr>,
    /// Wall-clock time in nanoseconds when Keplor ingested this event.
    pub ingested_at: i64,
}

/// Three-dimensional latency breakdown.
///
/// `ttft_ms` is `None` for non-streamed responses.  `total_ms` is always
/// set (request start → response fully consumed).  `time_to_close_ms` is
/// also optional — only applies to streaming responses where the
/// upstream closed the connection before the client did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct Latencies {
    /// Time to first byte (for streams: first payload chunk after
    /// headers).
    pub ttft_ms: Option<u32>,
    /// End-to-end request latency in milliseconds.
    pub total_ms: u32,
    /// Time the server spent keeping the stream open after the last
    /// chunk (trailer flush / graceful close).
    pub time_to_close_ms: Option<u32>,
}

/// 128-bit W3C trace-context trace id.
///
/// Displayed as 32 lowercase hex characters.  Parses both the bare hex
/// form and the `0x`-prefixed form.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub struct TraceId(pub [u8; 16]);

impl TraceId {
    /// Raw bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }
}

impl fmt::Display for TraceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for b in &self.0 {
            write!(f, "{b:02x}")?;
        }
        Ok(())
    }
}

impl FromStr for TraceId {
    type Err = CoreError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.strip_prefix("0x").unwrap_or(s);
        if s.len() != 32 {
            return Err(CoreError::InvalidTraceId(s.to_owned()));
        }
        let mut out = [0u8; 16];
        hex::decode_to_slice(s, &mut out).map_err(|_| CoreError::InvalidTraceId(s.to_owned()))?;
        Ok(Self(out))
    }
}

impl Serialize for TraceId {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for TraceId {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trace_id_display_is_32_hex_lower() {
        let t = TraceId([0xab; 16]);
        let s = t.to_string();
        assert_eq!(s.len(), 32);
        assert!(s.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        assert_eq!(s, "abababababababababababababababab");
    }

    #[test]
    fn trace_id_parse_roundtrip() {
        let orig = TraceId([1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]);
        let s = orig.to_string();
        let back: TraceId = s.parse().unwrap();
        assert_eq!(orig, back);
    }

    #[test]
    fn trace_id_accepts_0x_prefix() {
        let t: TraceId = "0x00112233445566778899aabbccddeeff".parse().unwrap();
        assert_eq!(t.0[0], 0x00);
        assert_eq!(t.0[15], 0xff);
    }

    #[test]
    fn trace_id_rejects_wrong_length() {
        assert!("abc".parse::<TraceId>().is_err());
        assert!("".parse::<TraceId>().is_err());
        // 31 chars
        assert!("abababababababababababababababab".len() == 32);
        assert!("ababababababababababababababab".parse::<TraceId>().is_err());
    }

    #[test]
    fn trace_id_serde_is_string() {
        let t: TraceId = "00112233445566778899aabbccddeeff".parse().unwrap();
        let j = serde_json::to_string(&t).unwrap();
        assert_eq!(j, "\"00112233445566778899aabbccddeeff\"");
        let back: TraceId = serde_json::from_str(&j).unwrap();
        assert_eq!(t, back);
    }

    #[test]
    fn latencies_default_is_zero() {
        let l = Latencies::default();
        assert_eq!(l.ttft_ms, None);
        assert_eq!(l.total_ms, 0);
        assert_eq!(l.time_to_close_ms, None);
    }

    #[test]
    fn latencies_serde_roundtrip() {
        let l = Latencies { ttft_ms: Some(25), total_ms: 300, time_to_close_ms: Some(5) };
        let j = serde_json::to_string(&l).unwrap();
        let back: Latencies = serde_json::from_str(&j).unwrap();
        assert_eq!(l, back);
    }

    #[test]
    fn llm_event_is_clone() {
        // Sanity check that every field is Clone-compatible.
        let e = LlmEvent {
            id: EventId::new(),
            ts_ns: 0,
            user_id: None,
            api_key_id: None,
            org_id: None,
            project_id: None,
            route_id: RouteId::from("chat"),
            provider: Provider::OpenAI,
            model: SmolStr::new("gpt-4o-mini"),
            model_family: Some(SmolStr::new("gpt-4")),
            endpoint: SmolStr::new("/v1/chat/completions"),
            method: Method::POST,
            http_status: Some(200),
            usage: Usage::default(),
            cost_nanodollars: 0,
            latency: Latencies::default(),
            flags: EventFlags::empty(),
            error: None,
            request_sha256: [0u8; 32],
            response_sha256: [0u8; 32],
            client_ip: None,
            user_agent: None,
            request_id: None,
            trace_id: None,
            source: None,
            ingested_at: 0,
        };
        let e2 = e.clone();
        assert_eq!(e.id, e2.id);
        assert_eq!(e.method, Method::POST);
    }
}
