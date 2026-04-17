//! Whitelist-based HTTP header scrubber.
//!
//! We store captured headers alongside the payload so an operator can
//! reconstruct what actually hit the upstream.  Anything that could leak
//! an API key or session cookie is stripped *before* storage, and the
//! stripping is whitelist-based so a provider adding a new exotic auth
//! header doesn't silently get persisted.

use http::{HeaderMap, HeaderName, HeaderValue};

/// The whitelist of header names that are safe to persist.
///
/// Lowercase only — [`HeaderName`] is canonicalised to lowercase by
/// `http`, so a case-insensitive compare is free.
const ALLOWED_HEADERS: &[&str] = &[
    // --- generic HTTP -----------------------------------------------------
    "accept",
    "accept-encoding",
    "accept-language",
    "cache-control",
    "content-encoding",
    "content-language",
    "content-length",
    "content-range",
    "content-type",
    "date",
    "etag",
    "expires",
    "if-match",
    "if-modified-since",
    "if-none-match",
    "last-modified",
    "pragma",
    "range",
    "referer",
    "server",
    "user-agent",
    "vary",
    "via",
    "warning",
    // --- rate-limit / retry (uniformly useful across providers) ----------
    "retry-after",
    "x-ratelimit-limit",
    "x-ratelimit-limit-requests",
    "x-ratelimit-limit-tokens",
    "x-ratelimit-remaining",
    "x-ratelimit-remaining-requests",
    "x-ratelimit-remaining-tokens",
    "x-ratelimit-reset",
    "x-ratelimit-reset-requests",
    "x-ratelimit-reset-tokens",
    // --- request / trace ids (observability, non-sensitive) --------------
    "x-request-id",
    "x-correlation-id",
    "x-amzn-requestid",
    "x-amzn-trace-id",
    "openai-organization",
    "openai-processing-ms",
    "openai-version",
    "openai-model",
    "anthropic-ratelimit-requests-limit",
    "anthropic-ratelimit-requests-remaining",
    "anthropic-ratelimit-requests-reset",
    "anthropic-ratelimit-tokens-limit",
    "anthropic-ratelimit-tokens-remaining",
    "anthropic-ratelimit-tokens-reset",
    "x-groq-region",
];

/// Return a new [`HeaderMap`] containing only the safe-to-persist headers
/// from `headers`.
///
/// Headers that match any of the following patterns are *always*
/// stripped, even if someone accidentally adds them to
/// [`ALLOWED_HEADERS`]:
///
/// - `authorization`
/// - `x-api-key`
/// - `api-key`
/// - `x-goog-api-key`
/// - `x-amz-security-token`
/// - `x-amz-signature`
/// - `x-amz-content-sha256`
/// - `x-amz-date`
/// - `cookie`
/// - `set-cookie`
/// - `proxy-authorization`
#[must_use]
pub fn sanitize_headers(headers: &HeaderMap) -> HeaderMap {
    let mut out = HeaderMap::with_capacity(headers.len());
    for name in ALLOWED_HEADERS {
        if is_always_banned(name) {
            // Defence-in-depth: never emit a banned header even if it
            // made it onto the whitelist by mistake.
            continue;
        }
        // A given header may appear multiple times (e.g. Set-Cookie).
        // `get_all` iterates all values for the same name.
        let Ok(parsed_name) = HeaderName::from_bytes(name.as_bytes()) else { continue };
        for value in headers.get_all(&parsed_name) {
            append(&mut out, &parsed_name, value);
        }
    }
    out
}

fn append(out: &mut HeaderMap, name: &HeaderName, value: &HeaderValue) {
    // Append with an owned name so multi-value headers coexist.
    out.append(name.clone(), value.clone());
}

fn is_always_banned(name: &str) -> bool {
    // Lowercase match — callers pass lowercase names from the whitelist.
    matches!(
        name,
        "authorization"
            | "proxy-authorization"
            | "x-api-key"
            | "api-key"
            | "x-goog-api-key"
            | "x-amz-security-token"
            | "x-amz-signature"
            | "x-amz-content-sha256"
            | "x-amz-date"
            | "x-amz-target"
            | "cookie"
            | "set-cookie"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build(headers: &[(&str, &str)]) -> HeaderMap {
        let mut m = HeaderMap::new();
        for (k, v) in headers {
            let name = HeaderName::from_bytes(k.as_bytes()).unwrap();
            let value = HeaderValue::from_str(v).unwrap();
            m.append(name, value);
        }
        m
    }

    #[test]
    fn strips_every_known_auth_header() {
        let incoming = build(&[
            ("Authorization", "Bearer sk-live-abc"),
            ("x-api-key", "sk-ant-xyz"),
            ("api-key", "azure-secret"),
            ("x-goog-api-key", "AIza-secret"),
            ("x-amz-security-token", "iam-token"),
            ("x-amz-signature", "sigv4-sig"),
            ("x-amz-content-sha256", "hash"),
            ("x-amz-date", "20260417T120000Z"),
            ("cookie", "sid=abc123"),
            ("set-cookie", "sid=abc123; Path=/"),
            ("proxy-authorization", "Basic xyz"),
        ]);
        let out = sanitize_headers(&incoming);
        assert!(out.is_empty(), "expected all auth headers stripped, got {out:?}");
    }

    #[test]
    fn preserves_whitelisted_headers() {
        let incoming = build(&[
            ("content-type", "application/json"),
            ("content-length", "123"),
            ("user-agent", "keplor-tests/0.1"),
            ("x-request-id", "req_abc"),
            ("x-ratelimit-remaining-tokens", "1000"),
            ("retry-after", "30"),
        ]);
        let out = sanitize_headers(&incoming);
        assert_eq!(out.get("content-type").unwrap(), "application/json");
        assert_eq!(out.get("content-length").unwrap(), "123");
        assert_eq!(out.get("user-agent").unwrap(), "keplor-tests/0.1");
        assert_eq!(out.get("x-request-id").unwrap(), "req_abc");
        assert_eq!(out.get("x-ratelimit-remaining-tokens").unwrap(), "1000");
        assert_eq!(out.get("retry-after").unwrap(), "30");
    }

    #[test]
    fn unknown_header_is_stripped_by_default() {
        let incoming = build(&[
            ("x-keplor-secret-experimental", "nope"),
            ("x-fancy-new-provider-key", "leak"),
        ]);
        let out = sanitize_headers(&incoming);
        assert!(out.is_empty());
    }

    #[test]
    fn multi_value_header_is_preserved_multi_value() {
        let mut m = HeaderMap::new();
        m.append(HeaderName::from_static("via"), HeaderValue::from_static("1.1 alpha"));
        m.append(HeaderName::from_static("via"), HeaderValue::from_static("1.1 beta"));
        let out = sanitize_headers(&m);
        assert_eq!(out.get_all("via").iter().count(), 2);
    }

    #[test]
    fn is_always_banned_is_a_strict_denylist() {
        assert!(is_always_banned("authorization"));
        assert!(is_always_banned("x-api-key"));
        assert!(is_always_banned("cookie"));
        assert!(!is_always_banned("content-type"));
    }
}
