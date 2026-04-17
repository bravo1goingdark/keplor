//! Route table: map incoming `Host` + path to an upstream target.
//!
//! The [`RouteTable`] is an ordered list of [`Route`] entries evaluated
//! first-match-wins.  Each entry matches on the `Host` header
//! (case-insensitive) and an optional path prefix.

use http::Uri;
use keplor_core::{Provider, RouteId};

use crate::config::RouteConfig;
use crate::error::ProxyError;

/// A resolved route mapping an incoming request to an upstream.
#[derive(Debug, Clone)]
pub struct Route {
    /// Logical name for this route (used in metrics and capture events).
    pub route_id: RouteId,
    /// Auto-detected provider for this upstream.
    pub provider: Option<Provider>,
    /// Base URL of the upstream target.
    pub upstream_url: Uri,
    /// The host pattern to match (lowercased).
    host: String,
    /// Optional path prefix to match (lowercased).
    path_prefix: Option<String>,
}

/// Ordered route table — first match wins.
#[derive(Debug, Clone)]
pub struct RouteTable {
    routes: Vec<Route>,
}

impl RouteTable {
    /// Build a route table from configuration entries.
    ///
    /// # Errors
    ///
    /// Returns [`ProxyError::Config`] if any upstream URL is invalid.
    pub fn from_config(configs: &[RouteConfig]) -> Result<Self, ProxyError> {
        let mut routes = Vec::with_capacity(configs.len());
        for cfg in configs {
            let upstream_url: Uri =
                cfg.upstream_url.parse().map_err(|e: http::uri::InvalidUri| {
                    ProxyError::Config(format!(
                        "route '{}': invalid upstream_url '{}': {e}",
                        cfg.name, cfg.upstream_url,
                    ))
                })?;

            let host_lower = cfg.host.to_ascii_lowercase();
            let provider = Provider::from_host_path(&host_lower, "/");

            routes.push(Route {
                route_id: RouteId::from(cfg.name.as_str()),
                provider,
                upstream_url,
                host: host_lower,
                path_prefix: cfg.path_prefix.as_ref().map(|p| p.to_ascii_lowercase()),
            });
        }
        Ok(Self { routes })
    }

    /// Resolve the first matching route for the given host and path.
    ///
    /// Both `host` and `path` are compared case-insensitively.  Returns
    /// `None` if no route matches.
    pub fn resolve(&self, host: &str, path: &str) -> Option<&Route> {
        let host_lower = host.to_ascii_lowercase();
        // Strip port from host if present (e.g. "localhost:8080" → "localhost").
        let host_no_port = host_lower.rsplit_once(':').map_or(host_lower.as_str(), |(h, _)| h);

        for route in &self.routes {
            let route_host_no_port =
                route.host.rsplit_once(':').map_or(route.host.as_str(), |(h, _)| h);

            if host_no_port != route_host_no_port {
                continue;
            }

            if let Some(prefix) = &route.path_prefix {
                let path_lower = path.to_ascii_lowercase();
                if !path_lower.starts_with(prefix.as_str()) {
                    continue;
                }
            }

            return Some(route);
        }
        None
    }

    /// Number of configured routes.
    pub fn len(&self) -> usize {
        self.routes.len()
    }

    /// Whether the route table is empty.
    pub fn is_empty(&self) -> bool {
        self.routes.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(name: &str, host: &str, path_prefix: Option<&str>, upstream: &str) -> RouteConfig {
        RouteConfig {
            name: name.to_owned(),
            host: host.to_owned(),
            path_prefix: path_prefix.map(String::from),
            upstream_url: upstream.to_owned(),
        }
    }

    #[test]
    fn exact_host_match() {
        let table = RouteTable::from_config(&[cfg(
            "openai",
            "api.openai.com",
            None,
            "https://api.openai.com",
        )])
        .unwrap();

        let route = table.resolve("api.openai.com", "/v1/chat/completions");
        assert!(route.is_some());
        assert_eq!(route.unwrap().route_id.as_str(), "openai");
    }

    #[test]
    fn case_insensitive_host() {
        let table = RouteTable::from_config(&[cfg(
            "openai",
            "api.openai.com",
            None,
            "https://api.openai.com",
        )])
        .unwrap();

        assert!(table.resolve("API.OPENAI.COM", "/v1/completions").is_some());
    }

    #[test]
    fn path_prefix_match() {
        let table = RouteTable::from_config(&[cfg(
            "anthropic-v1",
            "api.anthropic.com",
            Some("/v1/"),
            "https://api.anthropic.com",
        )])
        .unwrap();

        assert!(table.resolve("api.anthropic.com", "/v1/messages").is_some());
        assert!(table.resolve("api.anthropic.com", "/v2/something").is_none());
    }

    #[test]
    fn first_match_wins() {
        let table = RouteTable::from_config(&[
            cfg("specific", "api.openai.com", Some("/v1/chat"), "https://a.example.com"),
            cfg("fallback", "api.openai.com", None, "https://b.example.com"),
        ])
        .unwrap();

        let route = table.resolve("api.openai.com", "/v1/chat/completions").unwrap();
        assert_eq!(route.route_id.as_str(), "specific");

        let route = table.resolve("api.openai.com", "/v1/embeddings").unwrap();
        assert_eq!(route.route_id.as_str(), "fallback");
    }

    #[test]
    fn no_match_returns_none() {
        let table = RouteTable::from_config(&[cfg(
            "openai",
            "api.openai.com",
            None,
            "https://api.openai.com",
        )])
        .unwrap();

        assert!(table.resolve("api.anthropic.com", "/v1/messages").is_none());
    }

    #[test]
    fn host_with_port_stripped() {
        let table =
            RouteTable::from_config(&[cfg("local", "localhost", None, "http://127.0.0.1:11434")])
                .unwrap();

        assert!(table.resolve("localhost:8080", "/api/generate").is_some());
    }

    #[test]
    fn invalid_upstream_url_rejected() {
        let result =
            RouteTable::from_config(&[cfg("bad", "example.com", None, "not a valid url %%%")]);
        assert!(result.is_err());
    }

    #[test]
    fn empty_table() {
        let table = RouteTable::from_config(&[]).unwrap();
        assert!(table.is_empty());
        assert_eq!(table.len(), 0);
        assert!(table.resolve("anything", "/any").is_none());
    }

    #[test]
    fn provider_auto_detected() {
        let table = RouteTable::from_config(&[
            cfg("openai", "api.openai.com", None, "https://api.openai.com"),
            cfg("anthropic", "api.anthropic.com", None, "https://api.anthropic.com"),
        ])
        .unwrap();

        let openai = table.resolve("api.openai.com", "/v1/chat/completions").unwrap();
        assert!(matches!(openai.provider, Some(Provider::OpenAI)));

        let anthropic = table.resolve("api.anthropic.com", "/v1/messages").unwrap();
        assert!(matches!(anthropic.provider, Some(Provider::Anthropic)));
    }
}
