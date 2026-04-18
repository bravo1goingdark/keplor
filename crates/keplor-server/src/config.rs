//! Server configuration.

use std::net::SocketAddr;
use std::path::PathBuf;

use serde::Deserialize;

/// Top-level server configuration.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    /// HTTP listener settings.
    pub server: ListenConfig,
    /// Storage settings.
    pub storage: StorageConfig,
    /// Authentication settings.
    pub auth: AuthConfig,
    /// Pipeline tuning.
    pub pipeline: PipelineConfig,
    /// Idempotency cache settings.
    pub idempotency: IdempotencyConfig,
    /// Per-key rate limiting settings.
    pub rate_limit: RateLimitServerConfig,
    /// Optional TLS configuration. When present, the server listens with TLS.
    pub tls: Option<TlsConfig>,
}

/// TLS configuration for HTTPS listeners.
#[derive(Debug, Deserialize)]
pub struct TlsConfig {
    /// Path to PEM-encoded certificate chain file.
    pub cert_path: PathBuf,
    /// Path to PEM-encoded private key file.
    pub key_path: PathBuf,
}

/// HTTP listener configuration.
#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct ListenConfig {
    /// Address to bind to.
    pub listen_addr: SocketAddr,
    /// Graceful shutdown timeout in seconds (drain batch writer + checkpoint).
    pub shutdown_timeout_secs: u64,
    /// Per-request timeout in seconds. Slow requests are dropped with 408.
    pub request_timeout_secs: u64,
    /// Maximum concurrent connections. Beyond this, requests queue.
    pub max_connections: usize,
}

impl Default for ListenConfig {
    fn default() -> Self {
        Self {
            listen_addr: ([0, 0, 0, 0], 8080).into(),
            shutdown_timeout_secs: 25,
            request_timeout_secs: 30,
            max_connections: 10_000,
        }
    }
}

/// Storage configuration.
#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct StorageConfig {
    /// Path to the SQLite database file.
    pub db_path: PathBuf,
    /// Automatic GC: delete events older than this many days. 0 = disabled.
    pub retention_days: u64,
    /// WAL checkpoint interval in seconds. 0 = disabled.
    pub wal_checkpoint_secs: u64,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self { db_path: PathBuf::from("keplor.db"), retention_days: 90, wal_checkpoint_secs: 300 }
    }
}

/// Authentication configuration.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct AuthConfig {
    /// API keys that are allowed to ingest events.
    /// When empty, authentication is disabled (open access).
    pub api_keys: Vec<String>,
}

/// Idempotency cache configuration.
#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct IdempotencyConfig {
    /// Enable idempotency key support. Default: `true`.
    pub enabled: bool,
    /// Time-to-live for cached responses in seconds. Default: 300 (5 min).
    pub ttl_secs: u64,
    /// Maximum number of cached idempotency keys. Default: 100,000.
    pub max_entries: usize,
}

impl Default for IdempotencyConfig {
    fn default() -> Self {
        Self { enabled: true, ttl_secs: 300, max_entries: 100_000 }
    }
}

/// Per-key rate limiting configuration.
#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct RateLimitServerConfig {
    /// Enable rate limiting. Default: `false`.
    pub enabled: bool,
    /// Requests per second per API key. Default: 100.0.
    pub requests_per_second: f64,
    /// Burst capacity per API key. Default: 200.
    pub burst: usize,
}

impl Default for RateLimitServerConfig {
    fn default() -> Self {
        Self { enabled: false, requests_per_second: 100.0, burst: 200 }
    }
}

/// Pipeline tuning knobs.
#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct PipelineConfig {
    /// Maximum number of events per batch write.
    pub batch_size: usize,
    /// Maximum request body size in bytes.
    pub max_body_bytes: usize,
}

impl Default for PipelineConfig {
    fn default() -> Self {
        Self {
            batch_size: 64,
            max_body_bytes: 10 * 1024 * 1024, // 10 MB
        }
    }
}

impl ServerConfig {
    /// Load configuration from a TOML file with `KEPLOR_` env overrides.
    pub fn load(path: &std::path::Path) -> Result<Self, figment::Error> {
        use figment::providers::{Env, Format, Toml};
        use figment::Figment;

        Figment::new().merge(Toml::file(path)).merge(Env::prefixed("KEPLOR_").split("_")).extract()
    }

    /// Validate configuration values at startup.
    ///
    /// Returns a descriptive error for invalid combinations that would
    /// cause silent misbehavior at runtime.
    pub fn validate(&self) -> Result<(), String> {
        if self.pipeline.batch_size == 0 {
            return Err("pipeline.batch_size must be > 0".into());
        }
        if self.pipeline.batch_size > 100_000 {
            return Err(format!(
                "pipeline.batch_size = {} is dangerously large (max 100,000)",
                self.pipeline.batch_size
            ));
        }
        if self.pipeline.max_body_bytes == 0 {
            return Err("pipeline.max_body_bytes must be > 0".into());
        }
        if self.pipeline.max_body_bytes > 100 * 1024 * 1024 {
            return Err(format!(
                "pipeline.max_body_bytes = {} exceeds 100 MB safety limit",
                self.pipeline.max_body_bytes
            ));
        }
        if self.storage.db_path.as_os_str().is_empty() {
            return Err("storage.db_path must not be empty".into());
        }
        if self.server.request_timeout_secs == 0 || self.server.request_timeout_secs > 300 {
            return Err(format!(
                "server.request_timeout_secs = {} must be in [1, 300]",
                self.server.request_timeout_secs
            ));
        }
        if self.server.max_connections == 0 {
            return Err("server.max_connections must be > 0".into());
        }
        if let Some(tls) = &self.tls {
            if !tls.cert_path.exists() {
                return Err(format!("tls.cert_path does not exist: {}", tls.cert_path.display()));
            }
            if !tls.key_path.exists() {
                return Err(format!("tls.key_path does not exist: {}", tls.key_path.display()));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_sane() {
        let cfg = ServerConfig::default();
        assert_eq!(cfg.server.listen_addr.port(), 8080);
        assert_eq!(cfg.storage.db_path, PathBuf::from("keplor.db"));
        assert!(cfg.auth.api_keys.is_empty());
        assert_eq!(cfg.pipeline.batch_size, 64);
    }

    #[test]
    fn defaults_validate() {
        ServerConfig::default().validate().unwrap();
    }

    #[test]
    fn zero_batch_size_rejected() {
        let mut cfg = ServerConfig::default();
        cfg.pipeline.batch_size = 0;
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn huge_batch_size_rejected() {
        let mut cfg = ServerConfig::default();
        cfg.pipeline.batch_size = 200_000;
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn empty_db_path_rejected() {
        let mut cfg = ServerConfig::default();
        cfg.storage.db_path = PathBuf::new();
        assert!(cfg.validate().is_err());
    }
}
