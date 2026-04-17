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
