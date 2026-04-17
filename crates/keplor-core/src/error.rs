//! [`CoreError`] — errors this crate itself raises.
//! [`ProviderError`] — normalised upstream-provider errors with a
//! best-effort classifier ([`ProviderError::from_provider_response`]).

use std::time::Duration;

use serde::{Deserialize, Serialize};
use smol_str::SmolStr;
use thiserror::Error;

use crate::Provider;

/// Errors raised by `keplor-core` itself.
#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum CoreError {
    /// Bad provider identifier string (e.g. failed to parse a stored id).
    #[error("invalid provider id {0:?}")]
    InvalidProvider(SmolStr),
    /// A newtype-id [`std::str::FromStr`] parse failed.
    #[error("invalid {kind} id {value:?}")]
    InvalidId {
        /// Which id kind was being parsed — `"event"`, `"user"`, …
        kind: &'static str,
        /// The input string that failed to parse.
        value: String,
    },
    /// Bad trace id (not a 32-char lowercase hex string).
    #[error("invalid trace id {0:?}: expected 32 hex chars")]
    InvalidTraceId(String),
}

/// Normalised upstream error.  Serializable so we can write it into the
/// event row as-is.
#[derive(Error, Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProviderError {
    /// HTTP 429 or provider-specific "too many requests" shape.
    #[error("rate limited")]
    RateLimited {
        /// Retry-After hint from the response (seconds converted to
        /// [`Duration`]).
        retry_after: Option<Duration>,
    },
    /// HTTP 400 with a non-special error body.
    #[error("invalid request: {0}")]
    InvalidRequest(String),
    /// HTTP 401 / 403.
    #[error("authentication failed")]
    AuthFailed,
    /// Prompt exceeded the model's context window.
    #[error("context length exceeded (limit {limit})")]
    ContextLengthExceeded {
        /// Model context limit reported by the provider, if available.
        limit: u32,
    },
    /// Request was rejected by provider-side content policy.
    #[error("content filtered: {reason}")]
    ContentFiltered {
        /// Provider-supplied reason string (category, policy, …).
        reason: SmolStr,
    },
    /// 408 / 504 / upstream timed out.
    #[error("upstream timeout")]
    UpstreamTimeout,
    /// 502 / 503 / upstream unreachable.
    #[error("upstream unavailable")]
    UpstreamUnavailable,
    /// Anything we can't classify.
    #[error("{status}: {message}")]
    Other {
        /// HTTP status code.
        status: u16,
        /// Extracted error message, truncated to 256 chars.
        message: SmolStr,
    },
}

impl ProviderError {
    /// Classify a provider error response into a [`ProviderError`].
    ///
    /// Order of checks:
    ///
    /// 1. Status-code fast path (401 / 403 → [`AuthFailed`], 429 →
    ///    [`RateLimited`], 408 / 504 → [`UpstreamTimeout`],
    ///    502 / 503 → [`UpstreamUnavailable`]).
    /// 2. Provider-specific JSON error-body shape (extract error `type`
    ///    and `message` fields).
    /// 3. Generic fall-through to [`Other`].
    ///
    /// [`AuthFailed`]: ProviderError::AuthFailed
    /// [`RateLimited`]: ProviderError::RateLimited
    /// [`UpstreamTimeout`]: ProviderError::UpstreamTimeout
    /// [`UpstreamUnavailable`]: ProviderError::UpstreamUnavailable
    /// [`Other`]: ProviderError::Other
    #[must_use]
    pub fn from_provider_response(provider: &Provider, status: u16, body: &[u8]) -> Self {
        let (err_type, err_message, retry_after_secs, context_limit) =
            parse_error_body(provider, body);

        // 1. Status-code fast path.
        match status {
            401 | 403 => return Self::AuthFailed,
            429 => {
                return Self::RateLimited { retry_after: retry_after_secs.map(Duration::from_secs) }
            },
            408 | 504 => return Self::UpstreamTimeout,
            502 | 503 => return Self::UpstreamUnavailable,
            _ => {},
        }

        // 2. Provider-body classification.
        if let Some(err_type) = err_type.as_deref() {
            let t = err_type.to_ascii_lowercase();
            if t.contains("rate_limit")
                || t.contains("too_many_requests")
                || t.contains("throttling")
            {
                return Self::RateLimited {
                    retry_after: retry_after_secs.map(Duration::from_secs),
                };
            }
            if t.contains("auth") || t.contains("permission") || t == "unauthorized" {
                return Self::AuthFailed;
            }
            if t.contains("context_length")
                || t.contains("too_long")
                || t.contains("context_window")
            {
                return Self::ContextLengthExceeded { limit: context_limit.unwrap_or(0) };
            }
            if t.contains("content_filter") || t.contains("safety") || t.contains("blocked") {
                return Self::ContentFiltered {
                    reason: SmolStr::new(err_message.as_deref().unwrap_or(err_type)),
                };
            }
            if t.contains("timeout") {
                return Self::UpstreamTimeout;
            }
        }

        // 3. Fallback: InvalidRequest for 400, else Other.
        let message = err_message
            .as_deref()
            .unwrap_or_else(|| std::str::from_utf8(body).unwrap_or("<non-utf8 body>"));
        let message = truncate(message, 256);

        if status == 400 {
            Self::InvalidRequest(message.to_owned())
        } else {
            Self::Other { status, message: SmolStr::new(message) }
        }
    }
}

/// Try to pull (error_type, error_message, retry_after_secs, context_limit)
/// out of a provider-specific error body.  All fields are best-effort.
fn parse_error_body(
    provider: &Provider,
    body: &[u8],
) -> (Option<String>, Option<String>, Option<u64>, Option<u32>) {
    // Most providers return JSON; Cohere occasionally returns plain text.
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(body) else {
        return (None, None, None, None);
    };

    // OpenAI / Azure / Mistral / Groq / xAI / DeepSeek / OpenAI-compat:
    //   { "error": { "type": "...", "message": "...", "code": "..." } }
    // Anthropic happens to use the same `/error/type` / `/error/message`
    // nesting — the outer `"type":"error"` at root is ignored (it would
    // classify every Anthropic failure as a content filter match).
    let mut err_type = v.pointer("/error/type").and_then(|x| x.as_str()).map(str::to_owned);
    let mut err_msg = v.pointer("/error/message").and_then(|x| x.as_str()).map(str::to_owned);

    // Bedrock: { "__type": "ThrottlingException", "message": "..." }
    if err_type.is_none() {
        err_type = v.get("__type").and_then(|x| x.as_str()).map(str::to_owned);
    }
    if err_msg.is_none() {
        err_msg = v.get("message").and_then(|x| x.as_str()).map(str::to_owned);
    }
    // Gemini: { "error": { "status": "INVALID_ARGUMENT", "message": "...", "code": 400 } }
    if err_type.is_none() {
        err_type = v.pointer("/error/status").and_then(|x| x.as_str()).map(str::to_owned);
    }
    // Cohere: { "message": "rate limit exceeded" } — already handled above.
    // Ollama: { "error": "..." } — the `error` key is a *string*, not an object.
    if err_msg.is_none() {
        err_msg = v.get("error").and_then(|x| x.as_str()).map(str::to_owned);
    }

    let retry_after = v
        .pointer("/error/retry_after")
        .and_then(serde_json::Value::as_u64)
        .or_else(|| v.get("retry_after").and_then(serde_json::Value::as_u64));

    let context_limit = v
        .pointer("/error/param/limit")
        .and_then(serde_json::Value::as_u64)
        .or_else(|| v.pointer("/error/limit").and_then(serde_json::Value::as_u64))
        .and_then(|x| u32::try_from(x).ok());

    // `provider` is accepted to reserve signature space for
    // provider-specific quirks we may add later (e.g. Bedrock's
    // base64-wrapped inner error).
    let _ = provider;

    (err_type, err_msg, retry_after, context_limit)
}

fn truncate(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    // Walk back to a char boundary so we never slice through a codepoint.
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_error_display() {
        let e = CoreError::InvalidId { kind: "user", value: "".into() };
        assert_eq!(e.to_string(), "invalid user id \"\"");
    }

    #[test]
    fn status_code_fast_path() {
        let e = ProviderError::from_provider_response(&Provider::OpenAI, 401, b"");
        assert_eq!(e, ProviderError::AuthFailed);

        let e = ProviderError::from_provider_response(&Provider::OpenAI, 403, b"");
        assert_eq!(e, ProviderError::AuthFailed);

        let e = ProviderError::from_provider_response(&Provider::OpenAI, 504, b"");
        assert_eq!(e, ProviderError::UpstreamTimeout);

        let e = ProviderError::from_provider_response(&Provider::OpenAI, 503, b"");
        assert_eq!(e, ProviderError::UpstreamUnavailable);
    }

    #[test]
    fn openai_rate_limited_with_retry_after() {
        let body = br#"{
            "error": {
                "type": "rate_limit_exceeded",
                "message": "You exceeded your current quota",
                "retry_after": 60
            }
        }"#;
        let e = ProviderError::from_provider_response(&Provider::OpenAI, 429, body);
        assert_eq!(e, ProviderError::RateLimited { retry_after: Some(Duration::from_secs(60)) });
    }

    #[test]
    fn openai_context_length_exceeded() {
        let body = br#"{
            "error": {
                "type": "context_length_exceeded",
                "message": "This model's maximum context length is 8192 tokens.",
                "param": { "limit": 8192 }
            }
        }"#;
        let e = ProviderError::from_provider_response(&Provider::OpenAI, 400, body);
        assert_eq!(e, ProviderError::ContextLengthExceeded { limit: 8192 });
    }

    #[test]
    fn openai_content_filtered() {
        let body = br#"{
            "error": {
                "type": "content_filter",
                "message": "Your request was rejected as a result of our safety system"
            }
        }"#;
        let e = ProviderError::from_provider_response(&Provider::OpenAI, 400, body);
        match e {
            ProviderError::ContentFiltered { reason } => {
                assert!(reason.contains("safety"), "reason = {reason:?}");
            },
            other => panic!("expected ContentFiltered, got {other:?}"),
        }
    }

    #[test]
    fn anthropic_nested_error_shape() {
        let body = br#"{
            "type": "error",
            "error": {
                "type": "invalid_request_error",
                "message": "messages: at least one message is required"
            }
        }"#;
        let e = ProviderError::from_provider_response(&Provider::Anthropic, 400, body);
        match e {
            ProviderError::InvalidRequest(msg) => assert!(msg.contains("messages"), "msg = {msg}"),
            other => panic!("expected InvalidRequest, got {other:?}"),
        }
    }

    #[test]
    fn anthropic_overloaded() {
        let body = br#"{
            "type": "error",
            "error": { "type": "overloaded_error", "message": "Overloaded" }
        }"#;
        let e = ProviderError::from_provider_response(&Provider::Anthropic, 529, body);
        match e {
            ProviderError::Other { status, .. } => assert_eq!(status, 529),
            other => panic!("expected Other, got {other:?}"),
        }
    }

    #[test]
    fn bedrock_throttling_exception() {
        let body = br#"{
            "__type": "ThrottlingException",
            "message": "Rate exceeded"
        }"#;
        let e = ProviderError::from_provider_response(&Provider::Bedrock, 400, body);
        assert_eq!(e, ProviderError::RateLimited { retry_after: None });
    }

    #[test]
    fn gemini_invalid_argument() {
        let body = br#"{
            "error": {
                "code": 400,
                "status": "INVALID_ARGUMENT",
                "message": "Request contains an invalid argument."
            }
        }"#;
        let e = ProviderError::from_provider_response(&Provider::Gemini, 400, body);
        match e {
            ProviderError::InvalidRequest(msg) => {
                assert!(msg.contains("invalid argument"), "{msg}")
            },
            other => panic!("expected InvalidRequest, got {other:?}"),
        }
    }

    #[test]
    fn cohere_plain_message() {
        let body = br#"{"message":"invalid api token"}"#;
        let e = ProviderError::from_provider_response(&Provider::Cohere, 400, body);
        match e {
            ProviderError::InvalidRequest(m) => assert_eq!(m, "invalid api token"),
            other => panic!("expected InvalidRequest, got {other:?}"),
        }
    }

    #[test]
    fn ollama_error_field() {
        let body = br#"{"error":"model 'llama3' not found"}"#;
        let e = ProviderError::from_provider_response(&Provider::Ollama, 404, body);
        match e {
            ProviderError::Other { status, message } => {
                assert_eq!(status, 404);
                assert!(message.contains("llama3"));
            },
            other => panic!("expected Other, got {other:?}"),
        }
    }

    #[test]
    fn non_json_body_falls_back_to_other() {
        let body = b"<html>500 server error</html>";
        let e = ProviderError::from_provider_response(&Provider::OpenAI, 500, body);
        match e {
            ProviderError::Other { status, message } => {
                assert_eq!(status, 500);
                assert!(message.contains("500 server error"));
            },
            other => panic!("expected Other, got {other:?}"),
        }
    }

    #[test]
    fn truncate_is_utf8_safe() {
        // A multibyte char right at the boundary must not slice in half.
        let s = "é".repeat(300);
        let t = truncate(&s, 256);
        // Valid UTF-8 after truncation.
        assert!(std::str::from_utf8(t.as_bytes()).is_ok());
        assert!(t.len() <= 256);
    }

    #[test]
    fn message_is_bounded_to_256_chars() {
        let big = "x".repeat(10_000);
        let body = format!(r#"{{"error":{{"message":"{big}"}}}}"#);
        let e = ProviderError::from_provider_response(&Provider::OpenAI, 500, body.as_bytes());
        match e {
            ProviderError::Other { message, .. } => assert!(message.len() <= 256),
            other => panic!("expected Other, got {other:?}"),
        }
    }

    #[test]
    fn provider_error_serde_roundtrip() {
        let e = ProviderError::RateLimited { retry_after: Some(Duration::from_secs(30)) };
        let j = serde_json::to_string(&e).unwrap();
        let back: ProviderError = serde_json::from_str(&j).unwrap();
        assert_eq!(e, back);
    }
}
