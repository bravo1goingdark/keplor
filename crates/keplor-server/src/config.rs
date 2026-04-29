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
    /// Optional S3/R2 event archival configuration.
    #[cfg(feature = "s3")]
    pub archive: Option<ArchiveConfig>,
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
///
/// The on-disk layout is now a KeplorDB data directory — one segment
/// tree per retention tier under `{data_dir}/tier={name}/`. The
/// `db_path` name survives the cutover as a backwards-compatible alias
/// in deployed TOMLs; it points at a directory now, not a SQLite file.
#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct StorageConfig {
    /// Path to the KeplorDB data directory.
    #[serde(alias = "db_path")]
    pub data_dir: PathBuf,
    /// Automatic GC: delete events older than this many days. 0 = disabled.
    pub retention_days: u64,
    /// WAL checkpoint (segment rotation) interval in seconds. 0 = disabled.
    pub wal_checkpoint_secs: u64,
    /// Maximum database size in megabytes. 0 = unlimited.
    /// When exceeded, ingestion returns HTTP 507 until space is freed (by GC
    /// or manual deletion).
    pub max_db_size_mb: u64,
    /// GC run interval in seconds. Default: 3600 (1 hour). 0 = disabled.
    pub gc_interval_secs: u64,
    /// Events-per-shard before forced rotation. Default 500k.
    pub wal_max_events: u32,
    /// fsync interval for batched writes. Default 64.
    pub wal_sync_interval: u32,
    /// Byte threshold for batched fsync. Default 256 KiB.
    pub wal_sync_bytes: u64,
    /// Per-engine WAL shard count. Default 4.
    pub wal_shard_count: usize,
    /// Mmap'd segment file LRU cache capacity. Default 256.
    pub mmap_cache_capacity: usize,
    /// Days of historical segments to replay into the rollup store on
    /// open. Default 7.
    pub rollup_replay_days: u32,
    /// Cadence of the in-process rollup-refresh background loop.
    /// Lower values keep daily_rollups closer to live; higher values
    /// reduce CPU overhead from the rollup pass. Default 60 seconds.
    /// Range: 5–3600.
    pub rollup_loop_secs: u64,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            data_dir: PathBuf::from("./keplor_data"),
            retention_days: 90,
            wal_checkpoint_secs: 300,
            max_db_size_mb: 0,
            gc_interval_secs: 3600,
            wal_max_events: 500_000,
            wal_sync_interval: 64,
            wal_sync_bytes: 256 * 1024,
            wal_shard_count: 4,
            mmap_cache_capacity: 256,
            rollup_replay_days: 7,
            rollup_loop_secs: 60,
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
    /// Bounded channel capacity for the batch writer. Default: 32768.
    /// Higher values absorb traffic bursts at the cost of more memory
    /// and more events at risk if the process crashes before flushing.
    pub channel_capacity: usize,
    /// Reject ingest requests carrying `request_body` / `response_body`
    /// fields with HTTP 400. Default: `false` — those fields are
    /// dropped silently and a counter is incremented, preserving
    /// compatibility with clients that haven't yet migrated. Flip on
    /// once your fleet has stopped sending them.
    pub strict_schema: bool,
    /// Hard ceiling on how long a single ingest write may wait for
    /// the BatchWriter flush before the request returns 500.
    /// Bounds the worst-case request latency under back-pressure.
    /// Default: 10 seconds. Range: 1–300.
    pub write_timeout_secs: u64,
}

impl Default for PipelineConfig {
    fn default() -> Self {
        Self {
            batch_size: 64,
            max_body_bytes: 10 * 1024 * 1024, // 10 MB
            channel_capacity: 32_768,
            strict_schema: false,
            write_timeout_secs: 10,
        }
    }
}

/// S3/R2 event archival configuration.
///
/// When configured, old events are serialized to zstd-compressed JSONL
/// files and uploaded to the specified S3-compatible bucket.  Events are
/// deleted from SQLite after a confirmed upload.
///
/// ```toml
/// [archive]
/// bucket = "keplor-archive"
/// endpoint = "https://<account-id>.r2.cloudflarestorage.com"
/// region = "auto"
/// access_key_id = "your-access-key"
/// secret_access_key = "your-secret-key"
/// prefix = "events"
/// archive_after_days = 30
/// archive_threshold_mb = 500
/// archive_batch_size = 10000
/// ```
#[cfg(feature = "s3")]
#[derive(Debug, Clone, Deserialize)]
pub struct ArchiveConfig {
    /// S3 bucket name.
    pub bucket: String,
    /// S3 endpoint URL.
    pub endpoint: String,
    /// S3 region (e.g. `"auto"` for R2, `"us-east-1"` for AWS).
    pub region: String,
    /// Access key ID.
    pub access_key_id: String,
    /// Secret access key.
    pub secret_access_key: String,
    /// Key prefix in the bucket (e.g. `"events"`).
    #[serde(default)]
    pub prefix: String,
    /// Use path-style addressing (required for MinIO).
    #[serde(default)]
    pub path_style: bool,
    /// Archive events older than this many days. 0 = disabled.
    /// For sub-day granularity, use `archive_after_hours` instead.
    #[serde(default = "default_archive_after_days")]
    pub archive_after_days: u64,
    /// Archive events older than this many hours. 0 = use `archive_after_days`.
    /// Takes precedence over `archive_after_days` when non-zero.
    /// Set to `1` to keep only the last hour in SQLite.
    #[serde(default)]
    pub archive_after_hours: u64,
    /// Archive when SQLite exceeds this size (MB). 0 = disabled.
    #[serde(default)]
    pub archive_threshold_mb: u64,
    /// Maximum events per JSONL archive file.
    #[serde(default = "default_archive_batch_size")]
    pub archive_batch_size: usize,
    /// How often the archive loop runs in seconds. Default: 3600 (1 hour).
    #[serde(default = "default_archive_interval_secs")]
    pub archive_interval_secs: u64,
}

#[cfg(feature = "s3")]
fn default_archive_after_days() -> u64 {
    30
}

#[cfg(feature = "s3")]
fn default_archive_batch_size() -> usize {
    10_000
}

#[cfg(feature = "s3")]
fn default_archive_interval_secs() -> u64 {
    3600 // 1 hour
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
        if self.storage.data_dir.as_os_str().is_empty() {
            return Err("storage.data_dir must not be empty".into());
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
        if self.storage.wal_shard_count == 0 || self.storage.wal_shard_count > 64 {
            return Err(format!(
                "storage.wal_shard_count = {} must be in [1, 64]",
                self.storage.wal_shard_count
            ));
        }
        if self.pipeline.write_timeout_secs == 0 || self.pipeline.write_timeout_secs > 300 {
            return Err(format!(
                "pipeline.write_timeout_secs = {} must be in [1, 300]",
                self.pipeline.write_timeout_secs
            ));
        }
        if self.storage.rollup_loop_secs < 5 || self.storage.rollup_loop_secs > 3600 {
            return Err(format!(
                "storage.rollup_loop_secs = {} must be in [5, 3600]",
                self.storage.rollup_loop_secs
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

    /// Log warnings for configurations that are valid but risky.
    ///
    /// Call after [`ServerConfig::validate`] succeeds.
    pub fn warn_risky_defaults(&self) {
        if self.storage.max_db_size_mb == 0 {
            tracing::warn!(
                "storage.max_db_size_mb is 0 (unlimited) — the database can \
                 grow until disk is full. Set a limit for production deployments"
            );
        }

        #[cfg(feature = "s3")]
        if let Some(ref archive) = self.archive {
            let min_retention = self
                .retention
                .tiers
                .iter()
                .filter(|t| t.days > 0)
                .map(|t| t.days)
                .min()
                .unwrap_or(u64::MAX);

            if archive.archive_after_days > min_retention {
                tracing::warn!(
                    archive_after_days = archive.archive_after_days,
                    min_tier_retention = min_retention,
                    "archive_after_days ({}) > shortest tier retention ({} days) — \
                     GC will delete events before they can be archived. \
                     Lower archive_after_days or increase tier retention",
                    archive.archive_after_days,
                    min_retention,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_sane() {
        let cfg = ServerConfig::default();
        assert_eq!(cfg.server.listen_addr.port(), 8080);
        assert_eq!(cfg.storage.data_dir, PathBuf::from("./keplor_data"));
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
    fn empty_data_dir_rejected() {
        let mut cfg = ServerConfig::default();
        cfg.storage.data_dir = PathBuf::new();
        assert!(cfg.validate().is_err());
    }
}
