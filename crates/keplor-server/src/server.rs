//! HTTP server setup and lifecycle.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::error_handling::HandleErrorLayer;
use axum::extract::DefaultBodyLimit;
use axum::http::StatusCode;
use axum::middleware;
use axum::routing::{delete, get, post};
use axum::Router;
use metrics_exporter_prometheus::PrometheusHandle;
use smol_str::SmolStr;
use tokio::net::TcpListener;
use tower::ServiceBuilder;
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::trace::TraceLayer;

use crate::auth::{self, ApiKeySet, HotKeys};
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
    gc_interval_secs: u64,
    retention_tiers: Vec<crate::config::RetentionTier>,
    wal_checkpoint_secs: u64,
    pricing_refresh_interval_secs: u64,
    pricing_source_url: String,
    catalog_handle: crate::pipeline::SharedCatalog,
    #[cfg(feature = "s3")]
    archive_config: Option<crate::config::ArchiveConfig>,
    tls_config: Option<Arc<rustls::ServerConfig>>,
    /// Hot-swappable API key set. SIGHUP rebuilds and `store()`s a new
    /// `ApiKeySet` here without restart.
    hot_keys: Arc<HotKeys>,
    /// Path to the source TOML so SIGHUP can re-parse it. None when the
    /// server was built from in-memory config (tests).
    config_path: Option<std::path::PathBuf>,
}

impl PipelineServer {
    /// Build a new server from a pipeline, config, and Prometheus handle.
    ///
    /// Returns an error if TLS is configured with invalid cert/key files.
    pub fn new(
        pipeline: Pipeline,
        keys: ApiKeySet,
        config: &ServerConfig,
        metrics_handle: PrometheusHandle,
    ) -> Result<Self, std::io::Error> {
        let writer = pipeline.writer_arc();
        let store = pipeline.store_arc();
        let catalog_handle = pipeline.catalog_handle();
        let default_tier = SmolStr::new(&config.retention.default_tier);
        let state = AppState { pipeline, metrics_handle: Arc::new(metrics_handle), default_tier };

        // Spawn background rollup task — cadence governed by
        // storage.rollup_loop_secs (default 60s).
        let rollup_store = state.pipeline.store_arc();
        rollup::spawn_rollup_task(
            rollup_store,
            Duration::from_secs(config.storage.rollup_loop_secs),
        );

        // Wrap the key set in an ArcSwap so SIGHUP can hot-swap it.
        // The middleware loads the current snapshot on every request.
        let keys: Arc<HotKeys> = Arc::new(arc_swap::ArcSwap::from_pointee(keys));
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
            .route("/v1/events/export", get(routes::export_events))
            .route("/v1/events/{id}", delete(routes::delete_event))
            .route("/v1/events", delete(routes::delete_events_bulk))
            .route("/v1/quota", get(routes::query_quota))
            .route("/v1/rollups", get(routes::query_rollups))
            .route("/v1/stats", get(routes::query_stats))
            .layer(DefaultBodyLimit::max(body_limit));

        if let Some(rl) = rate_limiter {
            authed = authed.layer(middleware::from_fn(rate_limit::make_rate_limit_middleware(rl)));
        }

        let request_timeout = Duration::from_secs(config.server.request_timeout_secs);
        let max_connections = config.server.max_connections;

        // Timeout + concurrency limit applied only to authed routes.
        // Health and metrics are excluded so observability is never
        // starved when the connection pool is saturated.
        // Clone the keys handle for the middleware closure; the original
        // is retained on the PipelineServer for SIGHUP-driven reloads.
        let keys_for_mw = Arc::clone(&keys);
        let authed = authed
            .layer(middleware::from_fn(move |req, next| {
                let keys = Arc::clone(&keys_for_mw);
                auth::require_api_key(keys, req, next)
            }))
            .layer(
                ServiceBuilder::new()
                    .layer(HandleErrorLayer::new(|_: tower::BoxError| async {
                        StatusCode::REQUEST_TIMEOUT
                    }))
                    .layer(tower::timeout::TimeoutLayer::new(request_timeout))
                    .layer(tower::limit::ConcurrencyLimitLayer::new(max_connections)),
            )
            .with_state(state.clone());

        let public = Router::new()
            .route("/health", get(routes::health))
            .route("/metrics", get(routes::metrics_handler))
            .with_state(state);

        // Declare `request_id` as a span field so the
        // `propagate_request_id` middleware's `Span::current().record(...)`
        // call actually attaches the value (the default span built by
        // TraceLayer doesn't declare it, and `record` silently drops
        // updates to undeclared fields). All downstream
        // `#[tracing::instrument]` spans inherit through the active span
        // stack.
        let trace_layer =
            TraceLayer::new_for_http().make_span_with(|request: &axum::http::Request<_>| {
                tracing::info_span!(
                    "http_request",
                    method = %request.method(),
                    uri = %request.uri(),
                    request_id = tracing::field::Empty,
                )
            });

        let router = Router::new()
            .merge(authed)
            .merge(public)
            .layer(middleware::from_fn(request_id::propagate_request_id))
            .layer(trace_layer)
            .layer(build_cors_layer(&config.cors));

        let tls_config = match config.tls.as_ref() {
            Some(tls) => {
                use rustls_pki_types::pem::PemObject;
                use rustls_pki_types::{CertificateDer, PrivateKeyDer};

                let certs: Vec<CertificateDer<'static>> =
                    CertificateDer::pem_file_iter(&tls.cert_path)
                        .map_err(|e| {
                            std::io::Error::new(
                                std::io::ErrorKind::InvalidInput,
                                format!("failed to read TLS cert {}: {e}", tls.cert_path.display()),
                            )
                        })?
                        .filter_map(|c| c.ok())
                        .collect();

                let key = PrivateKeyDer::from_pem_file(&tls.key_path).map_err(|e| {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        format!("failed to read TLS key {}: {e}", tls.key_path.display()),
                    )
                })?;

                let server_config = rustls::ServerConfig::builder()
                    .with_no_client_auth()
                    .with_single_cert(certs, key)
                    .map_err(|e| {
                        std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            format!("invalid TLS config: {e}"),
                        )
                    })?;

                tracing::info!("TLS enabled");
                Some(Arc::new(server_config))
            },
            None => None,
        };

        Ok(Self {
            router,
            addr: config.server.listen_addr,
            shutdown_timeout: Duration::from_secs(config.server.shutdown_timeout_secs),
            writer,
            store,
            gc_retention_days: config.storage.retention_days,
            gc_interval_secs: config.storage.gc_interval_secs,
            retention_tiers: config.retention.tiers.clone(),
            wal_checkpoint_secs: config.storage.wal_checkpoint_secs,
            pricing_refresh_interval_secs: config.pricing.refresh_interval_secs,
            pricing_source_url: config.pricing.source_url.clone(),
            catalog_handle,
            #[cfg(feature = "s3")]
            archive_config: config.archive.clone(),
            tls_config,
            hot_keys: keys,
            // SIGHUP reload only works when we know which file to re-parse.
            // The CLI sets this; in-memory config (tests) leaves it None.
            config_path: None,
        })
    }

    /// Attach the source config path so SIGHUP knows which file to re-parse.
    /// Called by the CLI right after `new()`. Idempotent.
    pub fn with_config_path(mut self, path: std::path::PathBuf) -> Self {
        self.config_path = Some(path);
        self
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
        let gc_interval_secs = self.gc_interval_secs;
        let wal_checkpoint_secs = self.wal_checkpoint_secs;

        // Spawn archive + GC combined loop when archive is configured.
        // Archive runs BEFORE GC to prevent data loss when
        // archive_after_days > tier retention days.
        #[cfg(feature = "s3")]
        if let Some(ref archive_cfg) = self.archive_config {
            let s3_config = keplor_store::ArchiveS3Config {
                bucket: archive_cfg.bucket.clone(),
                endpoint: archive_cfg.endpoint.clone(),
                region: archive_cfg.region.clone(),
                access_key_id: archive_cfg.access_key_id.clone(),
                secret_access_key: archive_cfg.secret_access_key.clone(),
                prefix: archive_cfg.prefix.clone(),
                path_style: archive_cfg.path_style,
            };
            match keplor_store::Archiver::new(
                Arc::clone(&store),
                &s3_config,
                tokio::runtime::Handle::current(),
            ) {
                Ok(archiver) => {
                    // Validate S3 connectivity at startup — fail fast on
                    // bad credentials instead of discovering errors hours
                    // later on the first archive cycle.
                    if let Err(e) = archiver.probe() {
                        tracing::error!(
                            error = %e,
                            "S3 connectivity check failed — archival disabled. \
                             Check bucket, endpoint, and credentials in [archive]"
                        );
                    } else {
                        let archive_after_hours = if archive_cfg.archive_after_hours > 0 {
                            archive_cfg.archive_after_hours
                        } else {
                            archive_cfg.archive_after_days * 24
                        };
                        let archive_threshold_mb = archive_cfg.archive_threshold_mb;
                        let batch_size = archive_cfg.archive_batch_size;
                        let interval_secs = archive_cfg.archive_interval_secs;
                        let archive_store = Arc::clone(&store);
                        tracing::info!(
                            bucket = archive_cfg.bucket,
                            archive_after_hours,
                            archive_threshold_mb,
                            interval_secs,
                            "event archival configured — S3 connectivity verified"
                        );
                        tokio::spawn(archive_loop(
                            Arc::new(archiver),
                            archive_store,
                            archive_after_hours,
                            archive_threshold_mb,
                            batch_size,
                            interval_secs,
                        ));
                    }
                },
                Err(e) => {
                    tracing::error!(error = %e, "failed to initialize archiver — archival disabled");
                },
            }
        }

        // Spawn tiered GC task if retention tiers are configured.
        if gc_interval_secs > 0 && !self.retention_tiers.is_empty() {
            let gc_store = Arc::clone(&store);
            let tiers = self.retention_tiers.clone();
            tokio::spawn(gc_tiered_loop(gc_store, tiers, gc_interval_secs));
        } else if gc_interval_secs > 0 && gc_retention_days > 0 {
            // Legacy fallback: global retention with no tiers.
            let gc_store = Arc::clone(&store);
            tokio::spawn(gc_loop(gc_store, gc_retention_days, gc_interval_secs));
        }

        // Spawn WAL checkpoint task.
        if wal_checkpoint_secs > 0 {
            let ckpt_store = Arc::clone(&store);
            tokio::spawn(wal_checkpoint_loop(ckpt_store, wal_checkpoint_secs));
        }

        // Spawn the per-tier engine-stats sampler. Updates four
        // gauges per tier on each tick; bounded label cardinality
        // keeps Prometheus storage cheap.
        {
            let stats_store = Arc::clone(&store);
            tokio::spawn(engine_stats_loop(stats_store, Duration::from_secs(10)));
        }

        // Spawn the pricing-catalog refresh task when enabled. The
        // task fetches and atomically swaps a fresh catalog at the
        // configured cadence; on error the existing catalog stays in
        // place.
        if self.pricing_refresh_interval_secs > 0 {
            let catalog_handle = Arc::clone(&self.catalog_handle);
            let url = self.pricing_source_url.clone();
            let interval = Duration::from_secs(self.pricing_refresh_interval_secs);
            tokio::spawn(pricing_refresh_loop(catalog_handle, url, interval));
        }

        // SIGHUP handler: re-parse the config and hot-swap the API key
        // set without dropping in-flight requests. Only installed when a
        // config_path was attached via `with_config_path` — in-memory
        // configs (tests) skip this.
        if let Some(cfg_path) = self.config_path.clone() {
            let hot_keys = Arc::clone(&self.hot_keys);
            tokio::spawn(sighup_reload_loop(hot_keys, cfg_path));
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

    #[cfg(unix)]
    {
        let Ok(mut sigterm) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        else {
            // Fall back to SIGINT-only if SIGTERM registration fails.
            ctrl_c.await.ok();
            return;
        };
        tokio::select! {
            _ = ctrl_c => {},
            _ = sigterm.recv() => {},
        }
    }

    #[cfg(not(unix))]
    {
        ctrl_c.await.ok();
    }

    tracing::info!("shutdown signal received, stopping new connections...");
}

/// Periodically run tiered garbage collection — one pass per configured tier.
async fn gc_tiered_loop(
    store: Arc<keplor_store::Store>,
    tiers: Vec<crate::config::RetentionTier>,
    interval_secs: u64,
) {
    let interval = Duration::from_secs(interval_secs);
    loop {
        tokio::time::sleep(interval).await;
        let now_ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as i64;

        for tier in &tiers {
            if tier.days == 0 {
                continue; // 0 = keep forever
            }
            let cutoff_ns = now_ns - (tier.days as i64 * 86_400 * 1_000_000_000);
            let gc_store = Arc::clone(&store);
            let tier_name = tier.name.clone();
            match tokio::task::spawn_blocking(move || gc_store.gc_tier(&tier_name, cutoff_ns)).await
            {
                Ok(Ok(stats)) => {
                    if stats.events_deleted > 0 || stats.blobs_deleted > 0 {
                        tracing::info!(
                            tier = tier.name,
                            events = stats.events_deleted,
                            blobs = stats.blobs_deleted,
                            retention_days = tier.days,
                            "tiered gc completed"
                        );
                    }
                },
                Ok(Err(e)) => tracing::warn!(tier = tier.name, error = %e, "tiered gc failed"),
                Err(e) => tracing::warn!(tier = tier.name, error = %e, "tiered gc task panicked"),
            }
        }
    }
}

/// Periodically delete events older than `retention_days` and orphaned blobs.
/// Legacy fallback when no retention tiers are configured.
async fn gc_loop(store: Arc<keplor_store::Store>, retention_days: u64, interval_secs: u64) {
    let interval = Duration::from_secs(interval_secs);
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
/// Periodically archive old events to S3/R2.
///
/// Runs every `interval_secs` (default 3600 = 1 hour).
/// Checks both age and size triggers.
#[cfg(feature = "s3")]
async fn archive_loop(
    archiver: Arc<keplor_store::Archiver>,
    store: Arc<keplor_store::Store>,
    archive_after_hours: u64,
    archive_threshold_mb: u64,
    batch_size: usize,
    interval_secs: u64,
) {
    let interval = Duration::from_secs(interval_secs);
    loop {
        tokio::time::sleep(interval).await;

        let should_archive = {
            let mut trigger = false;

            // Age-based trigger.
            if archive_after_hours > 0 {
                trigger = true;
            }

            // Size-based trigger.
            if archive_threshold_mb > 0 {
                let threshold_bytes = archive_threshold_mb * 1024 * 1024;
                let db_size = {
                    let s = Arc::clone(&store);
                    tokio::task::spawn_blocking(move || s.db_size_bytes())
                        .await
                        .unwrap_or(Ok(0))
                        .unwrap_or(0)
                };
                if db_size >= threshold_bytes {
                    trigger = true;
                }
            }
            trigger
        };

        if !should_archive {
            continue;
        }

        let cutoff_ns = if archive_after_hours > 0 {
            let now_ns = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos() as i64;
            now_ns - (archive_after_hours as i64 * 3_600 * 1_000_000_000)
        } else {
            // Size-only trigger: archive everything older than 1 day.
            let now_ns = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos() as i64;
            now_ns - 86_400_000_000_000
        };

        let archiver = Arc::clone(&archiver);
        let bs = batch_size;
        match tokio::task::spawn_blocking(move || archiver.archive_old_events(cutoff_ns, bs)).await
        {
            Ok(Ok(result)) => {
                if result.events_archived > 0 {
                    tracing::info!(
                        events = result.events_archived,
                        files = result.files_uploaded,
                        compressed_bytes = result.compressed_bytes,
                        "archive cycle completed"
                    );
                }
            },
            Ok(Err(e)) => tracing::warn!(error = %e, "archive cycle failed"),
            Err(e) => tracing::warn!(error = %e, "archive task panicked"),
        }
    }
}

/// Periodically fetch the LiteLLM pricing catalog and atomically swap
/// it into the shared [`crate::pipeline::SharedCatalog`]. On fetch
/// or parse error the existing catalog stays in place — operators
/// see the failure via the `keplor_pricing_catalog_refresh_total`
/// counter and the `keplor_pricing_catalog_age_seconds` gauge.
///
/// The interval is jittered ±10 % so multi-replica deployments don't
/// stampede the upstream blob at the same wall-clock minute.
async fn pricing_refresh_loop(
    catalog: crate::pipeline::SharedCatalog,
    source_url: String,
    interval: Duration,
) {
    use std::sync::atomic::{AtomicI64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static LAST_REFRESH_NS: AtomicI64 = AtomicI64::new(0);
    let now_ns =
        SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos() as i64).unwrap_or(0);
    LAST_REFRESH_NS.store(now_ns, Ordering::Relaxed);

    // Background gauge that ticks once a minute reporting catalog age,
    // even when no refresh is in flight.
    {
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(60)).await;
                let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos() as i64);
                if let Ok(now) = now {
                    let last = LAST_REFRESH_NS.load(Ordering::Relaxed);
                    let age_secs = now.saturating_sub(last) / 1_000_000_000;
                    metrics::gauge!("keplor_pricing_catalog_age_seconds").set(age_secs as f64);
                }
            }
        });
    }

    loop {
        // Compute jittered sleep: interval ± 10%.
        let jitter_ns = {
            let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
            // Cheap PRNG: low bits of the nanosecond timestamp.
            let bits = now.as_nanos() as i64;
            let pct = (bits.rem_euclid(2001) - 1000) as f64 / 10_000.0; // ±10 %
            (interval.as_nanos() as f64 * pct) as i64
        };
        let sleep_ns =
            (interval.as_nanos() as i64).saturating_add(jitter_ns).max(60_000_000_000) as u64;
        tokio::time::sleep(Duration::from_nanos(sleep_ns)).await;

        match keplor_pricing::Catalog::fetch_latest(&source_url).await {
            Ok(fresh) => {
                catalog.store(Arc::new(fresh));
                let now_ns = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_nanos() as i64)
                    .unwrap_or(0);
                LAST_REFRESH_NS.store(now_ns, Ordering::Relaxed);
                metrics::counter!("keplor_pricing_catalog_refresh_total", "result" => "ok")
                    .increment(1);
                tracing::info!(
                    version = keplor_pricing::PRICING_CATALOG_VERSION,
                    "pricing catalog refreshed"
                );
            },
            Err(e) => {
                metrics::counter!("keplor_pricing_catalog_refresh_total", "result" => "error")
                    .increment(1);
                tracing::warn!(error = %e, "pricing catalog refresh failed; keeping existing catalog");
            },
        }
    }
}

/// Sample per-tier KeplorDB engine stats and publish them as Prometheus
/// gauges. Runs on a fixed cadence (default 10 s) so `/metrics` doesn't
/// pay the engine-walk cost per scrape. Each tick clones the engines
/// snapshot, queries cheap accessors (`segment_count`, `wal_count`,
/// `total_events`, `total_bytes`), and updates the four gauges.
async fn engine_stats_loop(store: Arc<keplor_store::Store>, interval: Duration) {
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // First .tick() returns immediately so the gauges land in
    // /metrics on the first scrape after server start, not one
    // interval in.
    loop {
        ticker.tick().await;
        for s in store.engine_stats() {
            metrics::gauge!("keplor_segments_total", "tier" => s.tier.to_string())
                .set(s.segment_count as f64);
            metrics::gauge!("keplor_wal_events", "tier" => s.tier.to_string())
                .set(s.wal_events as f64);
            metrics::gauge!("keplor_storage_events", "tier" => s.tier.to_string())
                .set(s.total_events as f64);
            metrics::gauge!("keplor_storage_bytes", "tier" => s.tier.to_string())
                .set(s.total_bytes as f64);
        }
    }
}

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

/// SIGHUP-driven hot-reload of the API key set.
///
/// On every SIGHUP: re-parse the TOML at `config_path`, build a fresh
/// `ApiKeySet` from `[auth]`, and atomically swap it into `hot_keys`.
/// In-flight requests holding an `ArcSwap::load()` snapshot finish
/// against the OLD set; new requests (after the store) use the NEW set.
/// No requests are dropped. Logs success or the parse error.
async fn sighup_reload_loop(hot_keys: Arc<HotKeys>, config_path: std::path::PathBuf) {
    let mut sighup = match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup()) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(error = %e, "failed to install SIGHUP handler — config reloads disabled");
            return;
        },
    };
    tracing::info!(config = %config_path.display(), "SIGHUP reload handler installed");
    while sighup.recv().await.is_some() {
        match crate::config::ServerConfig::load(&config_path) {
            Ok(cfg) => {
                let new_keys = ApiKeySet::from_config(
                    cfg.auth.api_keys.clone(),
                    cfg.auth.api_key_entries.clone(),
                    &cfg.retention.default_tier,
                );
                hot_keys.store(Arc::new(new_keys));
                tracing::info!("SIGHUP: api key set reloaded successfully");
                metrics::counter!("keplor_sighup_reloads_total", "outcome" => "success")
                    .increment(1);
            },
            Err(e) => {
                tracing::error!(error = %e, "SIGHUP: config parse failed — keys unchanged");
                metrics::counter!("keplor_sighup_reloads_total", "outcome" => "fail").increment(1);
            },
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
                Ok((tcp, addr)) => {
                    // 10s timeout prevents a slow/stalled client from
                    // blocking the accept loop indefinitely.
                    match tokio::time::timeout(Duration::from_secs(10), self.acceptor.accept(tcp))
                        .await
                    {
                        Ok(Ok(tls)) => return (tls, addr),
                        Ok(Err(e)) => {
                            tracing::debug!(error = %e, "TLS handshake failed");
                        },
                        Err(_) => {
                            tracing::debug!("TLS handshake timed out after 10s");
                        },
                    }
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
/// Install (or reuse) the global Prometheus recorder.
///
/// Safe to call multiple times (test binaries call once per spawned
/// server). The first call installs the global metrics recorder; every
/// subsequent call returns the same `PrometheusHandle`, so all callers
/// see the same counters / gauges through `/metrics`.
pub fn install_metrics_recorder() -> PrometheusHandle {
    use std::sync::OnceLock;
    static GLOBAL_HANDLE: OnceLock<PrometheusHandle> = OnceLock::new();
    GLOBAL_HANDLE
        .get_or_init(|| {
            // reason: install_recorder() can only fail when another
            // recorder has already been installed by a *different*
            // crate, which would mean the host has a conflicting
            // metrics setup. Letting it crash at startup is exactly
            // what we want — silently dropping metrics would be worse.
            #[allow(clippy::expect_used)]
            metrics_exporter_prometheus::PrometheusBuilder::new()
                .install_recorder()
                .expect("global metrics recorder already installed by another crate")
        })
        .clone()
}
