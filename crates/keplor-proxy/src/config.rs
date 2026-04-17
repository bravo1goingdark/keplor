//! Proxy configuration loaded via [`figment`].
//!
//! Configuration is layered: file (`keplor.toml`) → environment variables
//! (`KEPLOR_` prefix).  Sensible defaults are provided for all fields.

use std::net::SocketAddr;
use std::path::PathBuf;

use figment::providers::{Env, Format, Toml};
use figment::Figment;
use serde::Deserialize;

use crate::error::ProxyError;

/// Top-level proxy configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct ProxyConfig {
    /// Listener and TLS settings.
    #[serde(default)]
    pub server: ServerConfig,
    /// Upstream connection pool settings.
    #[serde(default)]
    pub upstream: UpstreamConfig,
    /// Capture pipeline settings.
    #[serde(default)]
    pub capture: CaptureConfig,
    /// Route table: maps incoming Host+path to upstream targets.
    #[serde(default)]
    pub routes: Vec<RouteConfig>,
}

/// Listener and TLS settings.
#[derive(Debug, Clone, Deserialize)]
pub struct ServerConfig {
    /// Address to bind the listener to.
    #[serde(default = "default_listen_addr")]
    pub listen_addr: SocketAddr,
    /// Path to the TLS certificate PEM file.  If absent, the proxy listens
    /// on plain HTTP (suitable for dev or when behind a TLS-terminating LB).
    pub tls_cert_path: Option<PathBuf>,
    /// Path to the TLS private key PEM file.
    pub tls_key_path: Option<PathBuf>,
    /// Maximum concurrent requests across all routes.
    #[serde(default = "default_max_concurrent")]
    pub max_concurrent_requests: usize,
    /// Graceful shutdown timeout in seconds.
    #[serde(default = "default_shutdown_timeout")]
    pub shutdown_timeout_secs: u64,
}

/// Upstream connection pool settings.
#[derive(Debug, Clone, Deserialize)]
pub struct UpstreamConfig {
    /// Connect timeout to the upstream in seconds.
    #[serde(default = "default_connect_timeout")]
    pub connect_timeout_secs: u64,
    /// Idle connection timeout in seconds.
    #[serde(default = "default_pool_idle_timeout")]
    pub pool_idle_timeout_secs: u64,
    /// Maximum idle connections per upstream host.
    #[serde(default = "default_pool_max_idle")]
    pub pool_max_idle_per_host: usize,
}

/// Capture pipeline settings.
#[derive(Debug, Clone, Deserialize)]
pub struct CaptureConfig {
    /// Bounded channel capacity for the tee → capture path.
    #[serde(default = "default_channel_capacity")]
    pub channel_capacity: usize,
    /// Whether capture is enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,
}

/// A single route entry mapping an incoming host (+ optional path prefix)
/// to an upstream URL.
#[derive(Debug, Clone, Deserialize)]
pub struct RouteConfig {
    /// Human-readable name for this route (used as [`RouteId`]).
    pub name: String,
    /// Incoming `Host` header to match (exact match, case-insensitive).
    pub host: String,
    /// Optional path prefix to match (e.g. `/v1/`).
    pub path_prefix: Option<String>,
    /// Upstream URL to forward to (e.g. `https://api.openai.com`).
    pub upstream_url: String,
}

impl ProxyConfig {
    /// Load configuration from `keplor.toml` + environment variables.
    ///
    /// # Errors
    ///
    /// Returns [`ProxyError::Config`] if the configuration cannot be loaded
    /// or deserialized.
    pub fn load() -> Result<Self, ProxyError> {
        Self::load_from("keplor.toml")
    }

    /// Load configuration from the given TOML file path + environment.
    ///
    /// # Errors
    ///
    /// Returns [`ProxyError::Config`] if the configuration cannot be loaded
    /// or deserialized.
    pub fn load_from(toml_path: &str) -> Result<Self, ProxyError> {
        Figment::new()
            .merge(Toml::file(toml_path))
            .merge(Env::prefixed("KEPLOR_").split("_"))
            .extract()
            .map_err(|e| ProxyError::Config(e.to_string()))
    }
}

// -- Defaults ----------------------------------------------------------------

fn default_listen_addr() -> SocketAddr {
    ([0, 0, 0, 0], 8080).into()
}

fn default_max_concurrent() -> usize {
    10_000
}

fn default_shutdown_timeout() -> u64 {
    25
}

fn default_connect_timeout() -> u64 {
    10
}

fn default_pool_idle_timeout() -> u64 {
    60
}

fn default_pool_max_idle() -> usize {
    32
}

fn default_channel_capacity() -> usize {
    4096
}

fn default_true() -> bool {
    true
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            listen_addr: default_listen_addr(),
            tls_cert_path: None,
            tls_key_path: None,
            max_concurrent_requests: default_max_concurrent(),
            shutdown_timeout_secs: default_shutdown_timeout(),
        }
    }
}

impl Default for UpstreamConfig {
    fn default() -> Self {
        Self {
            connect_timeout_secs: default_connect_timeout(),
            pool_idle_timeout_secs: default_pool_idle_timeout(),
            pool_max_idle_per_host: default_pool_max_idle(),
        }
    }
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self { channel_capacity: default_channel_capacity(), enabled: true }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialize_full_toml() {
        let toml = r#"
[server]
listen_addr = "127.0.0.1:9090"
tls_cert_path = "/etc/keplor/cert.pem"
tls_key_path = "/etc/keplor/key.pem"
max_concurrent_requests = 5000
shutdown_timeout_secs = 30

[upstream]
connect_timeout_secs = 5
pool_idle_timeout_secs = 120
pool_max_idle_per_host = 64

[capture]
channel_capacity = 8192
enabled = false

[[routes]]
name = "openai"
host = "api.openai.com"
upstream_url = "https://api.openai.com"

[[routes]]
name = "anthropic"
host = "api.anthropic.com"
path_prefix = "/v1/"
upstream_url = "https://api.anthropic.com"
"#;
        let config: ProxyConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.server.listen_addr, "127.0.0.1:9090".parse::<SocketAddr>().unwrap());
        assert_eq!(config.server.max_concurrent_requests, 5000);
        assert_eq!(config.upstream.pool_max_idle_per_host, 64);
        assert!(!config.capture.enabled);
        assert_eq!(config.routes.len(), 2);
        assert_eq!(config.routes[1].path_prefix.as_deref(), Some("/v1/"));
    }

    #[test]
    fn defaults_when_empty() {
        let config: ProxyConfig = toml::from_str("").unwrap();
        assert_eq!(config.server.listen_addr, SocketAddr::from(([0, 0, 0, 0], 8080)));
        assert_eq!(config.server.max_concurrent_requests, 10_000);
        assert_eq!(config.server.shutdown_timeout_secs, 25);
        assert_eq!(config.upstream.connect_timeout_secs, 10);
        assert_eq!(config.upstream.pool_idle_timeout_secs, 60);
        assert_eq!(config.upstream.pool_max_idle_per_host, 32);
        assert_eq!(config.capture.channel_capacity, 4096);
        assert!(config.capture.enabled);
        assert!(config.routes.is_empty());
    }
}
