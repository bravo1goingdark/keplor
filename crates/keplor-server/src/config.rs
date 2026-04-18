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
    /// CORS origin allowlist.
    pub cors: CorsConfig,
    /// Optional TLS configuration. When present, the server listens with TLS.
    pub tls: Option<TlsConfig>,
    /// Retention tier configuration.
    pub retention: RetentionConfig,
    /// Optional S3-compatible blob storage for request/response bodies.
    #[cfg(feature = "s3")]
    pub blob_storage: Option<keplor_store::blob::s3::S3BlobConfig>,
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
    /// Maximum database size in megabytes. 0 = unlimited.
    /// When exceeded, ingestion returns HTTP 507 until space is freed (by GC
    /// or manual deletion).
    pub max_db_size_mb: u64,
    /// Number of read connections in the pool. Default: 4.
    pub read_pool_size: usize,
    /// GC run interval in seconds. Default: 3600 (1 hour). 0 = disabled.
    pub gc_interval_secs: u64,
    /// Auto-offload blobs to external store when SQLite exceeds this size
    /// (in MB).  Requires `[blob_storage]` to be configured.
    /// 0 = manual mode (always use external store if configured, else embedded).
    pub blob_offload_threshold_mb: u64,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            db_path: PathBuf::from("keplor.db"),
            retention_days: 90,
            wal_checkpoint_secs: 300,
            max_db_size_mb: 0,
            read_pool_size: 4,
            gc_interval_secs: 3600,
            blob_offload_threshold_mb: 0,
        }
    }
}

/// Authentication configuration.
///
/// API keys can be specified in two formats:
///
/// **Simple format** (backward compatible):
/// ```toml
/// api_keys = ["prod:sk-abc123", "sk-bare-secret"]
/// ```
///
/// **Extended format** (with tier):
/// ```toml
/// [[auth.api_key_entries]]
/// id = "prod"
/// secret = "sk-abc123"
/// tier = "pro"
///
/// [[auth.api_key_entries]]
/// id = "dev"
/// secret = "sk-xyz789"
/// tier = "free"
/// ```
///
/// When both are provided, they are merged.  Simple-format keys
/// default to the `default_tier` from `[retention]`.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct AuthConfig {
    /// Simple API keys (`"id:secret"` or `"secret"`).
    /// These default to the `default_tier` from retention config.
    pub api_keys: Vec<String>,

    /// Extended API key entries with explicit tier assignment.
    pub api_key_entries: Vec<ApiKeyEntry>,
}

/// An API key entry with an explicit tier.
#[derive(Debug, Clone, Deserialize)]
pub struct ApiKeyEntry {
    /// Key identifier (shown in logs, stored in `api_key_id`).
    pub id: String,
    /// The secret value used in `Authorization: Bearer <secret>`.
    pub secret: String,
    /// Retention tier name (e.g. `"free"`, `"pro"`, `"team"`).
    /// Defaults to `"free"` if omitted.
    #[serde(default = "default_tier_name")]
    pub tier: String,
}

fn default_tier_name() -> String {
    "free".to_owned()
}

/// Retention tier configuration.
///
/// Defines named tiers with per-tier retention durations.  API keys
/// are mapped to tiers via [`AuthConfig`].  GC runs one pass per tier.
///
/// ```toml
/// [retention]
/// default_tier = "free"
///
/// [[retention.tiers]]
/// name = "free"
/// days = 7
///
/// [[retention.tiers]]
/// name = "pro"
/// days = 90
///
/// [[retention.tiers]]
/// name = "team"
/// days = 180
/// ```
#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct RetentionConfig {
    /// Tier name assigned to keys without an explicit tier.
    pub default_tier: String,
    /// Named retention tiers.
    pub tiers: Vec<RetentionTier>,
}

impl Default for RetentionConfig {
    fn default() -> Self {
        Self {
            default_tier: "free".to_owned(),
            tiers: vec![
                RetentionTier { name: "free".to_owned(), days: 7 },
                RetentionTier { name: "pro".to_owned(), days: 90 },
            ],
        }
    }
}

/// A named retention tier.
#[derive(Debug, Clone, Deserialize)]
pub struct RetentionTier {
    /// Tier name (e.g. `"free"`, `"pro"`, `"team"`, `"enterprise"`).
    pub name: String,
    /// How many days to retain events for this tier. 0 = keep forever.
    pub days: u64,
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

/// CORS configuration.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct CorsConfig {
    /// Allowed origins. When empty, only same-origin requests are allowed.
    /// Set to `["*"]` to allow all origins (not recommended in production).
    pub allowed_origins: Vec<String>,
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
        if self.storage.read_pool_size == 0 || self.storage.read_pool_size > 64 {
            return Err(format!(
                "storage.read_pool_size = {} must be in [1, 64]",
                self.storage.read_pool_size
            ));
        }
        if let Some(tls) = &self.tls {
            if !tls.cert_path.exists() {
                return Err(format!("tls.cert_path does not exist: {}", tls.cert_path.display()));
            }
            if !tls.key_path.exists() {
                return Err(format!("tls.key_path does not exist: {}", tls.key_path.display()));
            }
        }
        // Validate retention tiers.
        if self.retention.tiers.is_empty() {
            return Err("retention.tiers must have at least one tier".into());
        }
        for tier in &self.retention.tiers {
            if tier.name.is_empty() {
                return Err("retention tier name must not be empty".into());
            }
        }
        if !self.retention.tiers.iter().any(|t| t.name == self.retention.default_tier) {
            return Err(format!(
                "retention.default_tier '{}' does not match any defined tier",
                self.retention.default_tier
            ));
        }
        // Validate that extended key entries have non-empty fields.
        for entry in &self.auth.api_key_entries {
            if entry.id.is_empty() || entry.secret.is_empty() {
                return Err("api_key_entries: id and secret must not be empty".into());
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
