//! Hop-by-hop header stripping for HTTP forwarding.
//!
//! This module handles header manipulation for the *forwarding* path —
//! stripping headers that must not be forwarded through a proxy per
//! [RFC 9110 §7.6.1](https://www.rfc-editor.org/rfc/rfc9110#section-7.6.1).
//!
//! This is **distinct** from [`keplor_core::sanitize_headers`] which strips
//! auth headers for *storage*.  On the forwarding path, auth headers pass
//! through verbatim so the upstream receives them.

use http::header::{self, HeaderMap, HeaderValue};

/// Header names that must not be forwarded through an HTTP proxy.
const HOP_BY_HOP_NAMES: &[&str] = &[
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
];

/// Strip hop-by-hop headers from a request before forwarding upstream.
///
/// Auth headers (`authorization`, `x-api-key`, etc.) are **not** touched —
/// they pass through to the upstream verbatim.
pub fn strip_hop_by_hop_request(headers: &mut HeaderMap) {
    for name in HOP_BY_HOP_NAMES {
        headers.remove(*name);
    }
    headers.remove(header::HOST);
}

/// Strip hop-by-hop headers from a response before sending to the client.
pub fn strip_hop_by_hop_response(headers: &mut HeaderMap) {
    for name in HOP_BY_HOP_NAMES {
        headers.remove(*name);
    }
}

/// Replace the `Host` header with the upstream target's authority.
pub fn rewrite_host(headers: &mut HeaderMap, upstream_host: &str) {
    if let Ok(value) = HeaderValue::from_str(upstream_host) {
        headers.insert(header::HOST, value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::header::HeaderName;

    fn build(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut m = HeaderMap::new();
        for (k, v) in pairs {
            let name = HeaderName::from_bytes(k.as_bytes()).unwrap();
            let value = HeaderValue::from_str(v).unwrap();
            m.append(name, value);
        }
        m
    }

    #[test]
    fn strips_hop_by_hop_from_request() {
        let mut h = build(&[
            ("connection", "keep-alive"),
            ("keep-alive", "timeout=5"),
            ("proxy-authenticate", "Basic"),
            ("proxy-authorization", "Basic abc"),
            ("te", "trailers"),
            ("trailer", "Expires"),
            ("transfer-encoding", "chunked"),
            ("upgrade", "websocket"),
            ("host", "api.openai.com"),
            // These should survive:
            ("authorization", "Bearer sk-live-xxx"),
            ("x-api-key", "sk-ant-xyz"),
            ("content-type", "application/json"),
        ]);
        strip_hop_by_hop_request(&mut h);

        assert!(h.get("connection").is_none());
        assert!(h.get("keep-alive").is_none());
        assert!(h.get("proxy-authenticate").is_none());
        assert!(h.get("proxy-authorization").is_none());
        assert!(h.get("te").is_none());
        assert!(h.get("trailer").is_none());
        assert!(h.get("transfer-encoding").is_none());
        assert!(h.get("upgrade").is_none());
        assert!(h.get("host").is_none());

        assert_eq!(h.get("authorization").unwrap(), "Bearer sk-live-xxx");
        assert_eq!(h.get("x-api-key").unwrap(), "sk-ant-xyz");
        assert_eq!(h.get("content-type").unwrap(), "application/json");
    }

    #[test]
    fn strips_hop_by_hop_from_response() {
        let mut h = build(&[
            ("connection", "close"),
            ("transfer-encoding", "chunked"),
            ("content-type", "text/event-stream"),
            ("x-request-id", "req_123"),
        ]);
        strip_hop_by_hop_response(&mut h);

        assert!(h.get("connection").is_none());
        assert!(h.get("transfer-encoding").is_none());
        assert_eq!(h.get("content-type").unwrap(), "text/event-stream");
        assert_eq!(h.get("x-request-id").unwrap(), "req_123");
    }

    #[test]
    fn preserves_all_auth_headers() {
        let mut h = build(&[
            ("authorization", "Bearer sk-live-xxx"),
            ("x-api-key", "sk-ant-xyz"),
            ("api-key", "azure-key"),
            ("x-goog-api-key", "AIza-xxx"),
            ("x-amz-security-token", "iam-token"),
            ("x-amz-date", "20260417T120000Z"),
        ]);
        strip_hop_by_hop_request(&mut h);

        assert_eq!(h.get("authorization").unwrap(), "Bearer sk-live-xxx");
        assert_eq!(h.get("x-api-key").unwrap(), "sk-ant-xyz");
        assert_eq!(h.get("api-key").unwrap(), "azure-key");
        assert_eq!(h.get("x-goog-api-key").unwrap(), "AIza-xxx");
        assert_eq!(h.get("x-amz-security-token").unwrap(), "iam-token");
        assert_eq!(h.get("x-amz-date").unwrap(), "20260417T120000Z");
    }

    #[test]
    fn rewrite_host_replaces_value() {
        let mut h = build(&[("host", "proxy.local:8080")]);
        rewrite_host(&mut h, "api.openai.com");
        assert_eq!(h.get("host").unwrap(), "api.openai.com");
    }

    #[test]
    fn rewrite_host_inserts_when_missing() {
        let mut h = HeaderMap::new();
        rewrite_host(&mut h, "api.anthropic.com");
        assert_eq!(h.get("host").unwrap(), "api.anthropic.com");
    }
}
