//! HTTP server setup and lifecycle.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::error_handling::HandleErrorLayer;
use axum::extract::DefaultBodyLimit;
use axum::http::StatusCode;
use axum::middleware;
use axum::routing::{get, post};
use axum::Router;
use metrics_exporter_prometheus::PrometheusHandle;
use tokio::net::TcpListener;
use tower::ServiceBuilder;
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::trace::TraceLayer;

use crate::auth::{self, ApiKeySet};
use crate::config::ServerConfig;
use crate::pipeline::Pipeline;
use crate::rate_limit::{self, RateLimitConfig, RateLimiter};
use crate::request_id;
use crate::rollup;
use crate::routes::{self, AppState};

/// The Keplor ingestion server.
pub struct PipelineServer {
    router: Router,
    addr: SocketAddr,
    shutdown_timeout: Duration,
    writer: Arc<keplor_store::BatchWriter>,
    store: Arc<keplor_store::Store>,
    gc_retention_days: u64,
    wal_checkpoint_secs: u64,
    tls_config: Option<Arc<rustls::ServerConfig>>,
}

impl PipelineServer {
    /// Build a new server from a pipeline, config, and Prometheus handle.
    pub fn new(
        pipeline: Pipeline,
        keys: ApiKeySet,
        config: &ServerConfig,
        metrics_handle: PrometheusHandle,
    ) -> Self {
        let writer = pipeline.writer_arc();
        let store = pipeline.store_arc();
        let state = AppState { pipeline, metrics_handle: Arc::new(metrics_handle) };

        // Spawn background rollup task — refreshes today's daily_rollups
        // every 60s so aggregation queries stay current.
        let rollup_store = state.pipeline.store_arc();
        rollup::spawn_rollup_task(rollup_store, Duration::from_secs(60));

        let keys = Arc::new(keys);
        let body_limit = config.pipeline.max_body_bytes;

        // Build optional rate limiter.
        let rate_limiter: Option<Arc<RateLimiter>> = if config.rate_limit.enabled {
            let rl = RateLimiter::new(RateLimitConfig {
                requests_per_second: config.rate_limit.requests_per_second,
                burst: config.rate_limit.burst,
            });
            tracing::info!(
                rps = config.rate_limit.requests_per_second,
                burst = config.rate_limit.burst,
                "per-key rate limiting enabled"
            );
            Some(Arc::new(rl))
        } else {
            None
        };

        let mut authed = Router::new()
            .route("/v1/events", post(routes::ingest_single))
            .route("/v1/events/batch", post(routes::ingest_batch))
            .route("/v1/events", get(routes::query_events))
            .route("/v1/quota", get(routes::query_quota))
            .route("/v1/rollups", get(routes::query_rollups))
            .route("/v1/stats", get(routes::query_stats))
            .layer(DefaultBodyLimit::max(body_limit));

        if let Some(rl) = rate_limiter {
            authed = authed.layer(middleware::from_fn(rate_limit::make_rate_limit_middleware(rl)));
        }

        let authed = authed
            .layer(middleware::from_fn(move |req, next| {
                let keys = Arc::clone(&keys);
                auth::require_api_key(keys, req, next)
            }))
            .with_state(state.clone());

        let public = Router::new()
            .route("/health", get(routes::health))
            .route("/metrics", get(routes::metrics_handler))
            .with_state(state);

        let request_timeout = Duration::from_secs(config.server.request_timeout_secs);
        let max_connections = config.server.max_connections;

        // Timeout + concurrency limit applied to the full router.
        // Health and metrics endpoints are included — the concurrency
        // limit is high enough (default 10,000) that health probes
        // should not be blocked under normal conditions.
        let router = Router::new()
            .merge(authed)
            .merge(public)
            .layer(
                ServiceBuilder::new()
                    .layer(HandleErrorLayer::new(|_: tower::BoxError| async {
                        StatusCode::REQUEST_TIMEOUT
                    }))
                    .layer(tower::timeout::TimeoutLayer::new(request_timeout))
                    .layer(tower::limit::ConcurrencyLimitLayer::new(max_connections)),
            )
            .layer(middleware::from_fn(request_id::propagate_request_id))
            .layer(TraceLayer::new_for_http())
            .layer(build_cors_layer(&config.cors));

        let tls_config = config.tls.as_ref().map(|tls| {
            use rustls_pki_types::pem::PemObject;
            use rustls_pki_types::{CertificateDer, PrivateKeyDer};

            let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_file_iter(&tls.cert_path)
                .unwrap_or_else(|e| panic!("failed to read cert {}: {e}", tls.cert_path.display()))
                .filter_map(|c| c.ok())
                .collect();

            let key = PrivateKeyDer::from_pem_file(&tls.key_path)
                .unwrap_or_else(|e| panic!("failed to read key {}: {e}", tls.key_path.display()));

            let server_config = rustls::ServerConfig::builder()
                .with_no_client_auth()
                .with_single_cert(certs, key)
                .unwrap_or_else(|e| panic!("invalid TLS config: {e}"));

            Arc::new(server_config)
        });

        if tls_config.is_some() {
            tracing::info!("TLS enabled");
        }

        Self {
            router,
            addr: config.server.listen_addr,
            shutdown_timeout: Duration::from_secs(config.server.shutdown_timeout_secs),
            writer,
            store,
            gc_retention_days: config.storage.retention_days,
            wal_checkpoint_secs: config.storage.wal_checkpoint_secs,
            tls_config,
        }
    }

    /// Start the server and block until shutdown.
    ///
    /// On SIGINT/SIGTERM:
    /// 1. Stops accepting new connections (graceful shutdown)
    /// 2. Drains the batch writer (flushes pending events)
    /// 3. Runs a WAL checkpoint
    pub async fn run(self) -> Result<(), std::io::Error> {
        let listener = TcpListener::bind(self.addr).await?;
        tracing::info!(addr = %self.addr, "keplor server listening");
        self.run_inner(listener).await
    }

    /// Start the server on an already-bound listener (for tests).
    pub async fn run_on(self, listener: TcpListener) -> Result<(), std::io::Error> {
        let addr = listener.local_addr()?;
        tracing::info!(addr = %addr, "keplor server listening");
        self.run_inner(listener).await
    }

    async fn run_inner(self, listener: TcpListener) -> Result<(), std::io::Error> {
        let writer = Arc::clone(&self.writer);
        let store = Arc::clone(&self.store);
        let shutdown_timeout = self.shutdown_timeout;
        let gc_retention_days = self.gc_retention_days;
        let wal_checkpoint_secs = self.wal_checkpoint_secs;

        // Spawn automated GC task if retention is configured.
        if gc_retention_days > 0 {
            let gc_store = Arc::clone(&store);
            tokio::spawn(gc_loop(gc_store, gc_retention_days));
        }

        // Spawn WAL checkpoint task.
        if wal_checkpoint_secs > 0 {
            let ckpt_store = Arc::clone(&store);
            tokio::spawn(wal_checkpoint_loop(ckpt_store, wal_checkpoint_secs));
        }

        if let Some(tls_config) = &self.tls_config {
            let tls_listener = TlsListener::new(listener, tls_config.clone());
            axum::serve(tls_listener, self.router)
                .with_graceful_shutdown(shutdown_signal())
                .await?;
        } else {
            axum::serve(listener, self.router).with_graceful_shutdown(shutdown_signal()).await?;
        }

        // ── Post-shutdown cleanup ────────────────────────────────
        tracing::info!("draining batch writer...");
        let drained = writer.shutdown(shutdown_timeout).await;
        if drained {
            tracing::info!("batch writer drained successfully");
        } else {
            tracing::warn!(
                timeout_secs = shutdown_timeout.as_secs(),
                "batch writer drain timed out — some events may be lost"
            );
        }

        // Final WAL checkpoint.
        let ckpt_store = store;
        if let Err(e) = tokio::task::spawn_blocking(move || ckpt_store.wal_checkpoint()).await {
            tracing::warn!(error = %e, "final WAL checkpoint failed");
        }

        tracing::info!("keplor shut down cleanly");
        Ok(())
    }
}

/// Build a CORS layer from the configuration.
///
/// - Empty `allowed_origins`: no `Access-Control-Allow-Origin` header is
///   sent, so browsers enforce same-origin policy (restrictive default).
/// - `["*"]`: equivalent to `CorsLayer::permissive()`.
/// - Explicit list: only those origins are allowed.
fn build_cors_layer(config: &crate::config::CorsConfig) -> CorsLayer {
    if config.allowed_origins.is_empty() {
        // No origins configured — restrictive default.
        CorsLayer::new()
    } else if config.allowed_origins.len() == 1 && config.allowed_origins[0] == "*" {
        CorsLayer::permissive()
    } else {
        let origins: Vec<http::HeaderValue> =
            config.allowed_origins.iter().filter_map(|o| o.parse().ok()).collect();
        CorsLayer::new()
            .allow_origin(AllowOrigin::list(origins))
            .allow_methods([http::Method::GET, http::Method::POST, http::Method::OPTIONS])
            .allow_headers([http::header::CONTENT_TYPE, http::header::AUTHORIZATION])
    }
}

async fn shutdown_signal() {
    let ctrl_c = tokio::signal::ctrl_c();
    tokio::pin!(ctrl_c);
    ctrl_c.await.ok();
    tracing::info!("shutdown signal received, stopping new connections...");
}

/// Periodically delete events older than `retention_days` and orphaned blobs.
async fn gc_loop(store: Arc<keplor_store::Store>, retention_days: u64) {
    let interval = Duration::from_secs(3600); // Run every hour.
    loop {
        tokio::time::sleep(interval).await;
        let cutoff_ns = {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos() as i64;
            now - (retention_days as i64 * 86400 * 1_000_000_000)
        };

        let gc_store = Arc::clone(&store);
        match tokio::task::spawn_blocking(move || gc_store.gc_expired(cutoff_ns)).await {
            Ok(Ok(stats)) => {
                if stats.events_deleted > 0 || stats.blobs_deleted > 0 {
                    tracing::info!(
                        events = stats.events_deleted,
                        blobs = stats.blobs_deleted,
                        retention_days,
                        "gc completed"
                    );
                }
            },
            Ok(Err(e)) => tracing::warn!(error = %e, "gc failed"),
            Err(e) => tracing::warn!(error = %e, "gc task panicked"),
        }
    }
}

/// Periodically checkpoint the WAL to keep it from growing unbounded.
async fn wal_checkpoint_loop(store: Arc<keplor_store::Store>, interval_secs: u64) {
    let interval = Duration::from_secs(interval_secs);
    loop {
        tokio::time::sleep(interval).await;
        let ckpt_store = Arc::clone(&store);
        if let Err(e) = tokio::task::spawn_blocking(move || ckpt_store.wal_checkpoint()).await {
            tracing::warn!(error = %e, "wal checkpoint failed");
        }
    }
}

// ── TLS listener adapter ───────────────────────────────────────────────

/// A TLS-wrapping listener that performs the TLS handshake on each accepted
/// TCP connection before handing it to axum.
struct TlsListener {
    inner: TcpListener,
    acceptor: tokio_rustls::TlsAcceptor,
}

impl TlsListener {
    fn new(listener: TcpListener, config: Arc<rustls::ServerConfig>) -> Self {
        Self { inner: listener, acceptor: tokio_rustls::TlsAcceptor::from(config) }
    }
}

impl axum::serve::Listener for TlsListener {
    type Io = tokio_rustls::server::TlsStream<tokio::net::TcpStream>;
    type Addr = std::net::SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            match self.inner.accept().await {
                Ok((tcp, addr)) => match self.acceptor.accept(tcp).await {
                    Ok(tls) => return (tls, addr),
                    Err(e) => {
                        tracing::debug!(error = %e, "TLS handshake failed");
                        continue;
                    },
                },
                Err(e) => {
                    tracing::error!(error = %e, "TCP accept failed");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                },
            }
        }
    }

    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        self.inner.local_addr()
    }
}

/// Install the Prometheus metrics recorder globally and return the handle.
///
/// Safe to call multiple times (e.g. in tests) — subsequent calls return
/// a standalone handle.
pub fn install_metrics_recorder() -> PrometheusHandle {
    let builder = metrics_exporter_prometheus::PrometheusBuilder::new();
    match builder.install_recorder() {
        Ok(handle) => handle,
        Err(_) => {
            // Recorder already installed (common in tests). Build a
            // standalone recorder and return its handle.
            let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
            recorder.handle()
        },
    }
}
