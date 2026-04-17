//! Upstream HTTPS client pool.
//!
//! Wraps a [`hyper_util::client::legacy::Client`] with rustls + HTTP/2
//! support.  The client pools connections per-host internally; a single
//! instance handles all upstream providers.

use std::sync::Arc;
use std::time::Duration;

use arc_swap::ArcSwap;
use hyper_rustls::HttpsConnector;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;

use crate::config::UpstreamConfig;
use crate::error::ProxyError;

/// The concrete client type used by the proxy.
pub type UpstreamClient =
    Client<HttpsConnector<HttpConnector>, crate::tee::TeeBody<axum::body::Body>>;

/// Connection pool for upstream HTTPS requests.
///
/// Backed by a single [`Client`] that pools connections per-host.
/// The client is wrapped in [`ArcSwap`] for future hot-reload of TLS
/// configuration without restarting the process.
#[derive(Clone)]
pub struct UpstreamPool {
    client: Arc<ArcSwap<UpstreamClient>>,
}

impl UpstreamPool {
    /// Build a new upstream pool from configuration.
    ///
    /// # Errors
    ///
    /// Returns [`ProxyError::Tls`] if the TLS configuration fails.
    pub fn new(config: &UpstreamConfig) -> Result<Self, ProxyError> {
        let client = build_client(config)?;
        Ok(Self { client: Arc::new(ArcSwap::from_pointee(client)) })
    }

    /// Get a guard to the current client.
    pub fn client(&self) -> arc_swap::Guard<Arc<UpstreamClient>> {
        self.client.load()
    }

    /// Hot-swap the underlying client (e.g. after TLS config reload).
    pub fn swap(&self, config: &UpstreamConfig) -> Result<(), ProxyError> {
        let new_client = build_client(config)?;
        self.client.store(Arc::new(new_client));
        Ok(())
    }
}

fn build_client(config: &UpstreamConfig) -> Result<UpstreamClient, ProxyError> {
    let provider = rustls::crypto::aws_lc_rs::default_provider();
    let verifier = rustls::client::WebPkiServerVerifier::builder_with_provider(
        Arc::new(root_store()),
        Arc::new(provider.clone()),
    )
    .build()
    .map_err(|e| ProxyError::Config(format!("webpki verifier: {e}")))?;

    let tls = rustls::ClientConfig::builder_with_provider(Arc::new(provider))
        .with_safe_default_protocol_versions()
        .map_err(|e| ProxyError::Config(format!("tls protocol versions: {e}")))?
        .with_webpki_verifier(verifier)
        .with_no_client_auth();

    let https = hyper_rustls::HttpsConnectorBuilder::new()
        .with_tls_config(tls)
        .https_or_http()
        .enable_all_versions()
        .build();

    let client = Client::builder(TokioExecutor::new())
        .pool_idle_timeout(Duration::from_secs(config.pool_idle_timeout_secs))
        .pool_max_idle_per_host(config.pool_max_idle_per_host)
        .build(https);

    Ok(client)
}

fn root_store() -> rustls::RootCertStore {
    let mut store = rustls::RootCertStore::empty();
    store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    store
}

impl std::fmt::Debug for UpstreamPool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UpstreamPool").finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn construction_succeeds() {
        let config = UpstreamConfig::default();
        let pool = UpstreamPool::new(&config);
        assert!(pool.is_ok());
    }

    #[test]
    fn hot_swap_succeeds() {
        let config = UpstreamConfig::default();
        let pool = UpstreamPool::new(&config).unwrap();
        assert!(pool.swap(&config).is_ok());
    }
}
