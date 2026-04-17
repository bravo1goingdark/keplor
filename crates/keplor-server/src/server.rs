//! HTTP server setup and lifecycle.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::DefaultBodyLimit;
use axum::middleware;
use axum::routing::{get, post};
use axum::Router;
use metrics_exporter_prometheus::PrometheusHandle;
use tokio::net::TcpListener;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

use crate::auth::{self, ApiKeySet};
use crate::config::ServerConfig;
use crate::pipeline::Pipeline;
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
        let authed = Router::new()
            .route("/v1/events", post(routes::ingest_single))
            .route("/v1/events/batch", post(routes::ingest_batch))
            .route("/v1/events", get(routes::query_events))
            .route("/v1/quota", get(routes::query_quota))
            .route("/v1/rollups", get(routes::query_rollups))
            .route("/v1/stats", get(routes::query_stats))
            .layer(DefaultBodyLimit::max(body_limit))
            .layer(middleware::from_fn(move |req, next| {
                let keys = Arc::clone(&keys);
                auth::require_api_key(keys, req, next)
            }))
            .with_state(state.clone());

        let public = Router::new()
            .route("/health", get(routes::health))
            .route("/metrics", get(routes::metrics_handler))
            .with_state(state);

        let router = Router::new()
            .merge(authed)
            .merge(public)
            .layer(TraceLayer::new_for_http())
            .layer(CorsLayer::permissive());

        Self {
            router,
            addr: config.server.listen_addr,
            shutdown_timeout: Duration::from_secs(config.server.shutdown_timeout_secs),
            writer,
            store,
            gc_retention_days: config.storage.retention_days,
            wal_checkpoint_secs: config.storage.wal_checkpoint_secs,
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

        axum::serve(listener, self.router).with_graceful_shutdown(shutdown_signal()).await?;

        // ── Post-shutdown cleanup ────────────────────────────────
        tracing::info!("draining batch writer...");
        let drain_result = tokio::time::timeout(shutdown_timeout, async {
            // Drop our Arc so the writer's internal Sender can close.
            // The flush_loop will drain remaining events when it sees None.
            drop(writer);
            // Give the flush loop time to complete.
            tokio::time::sleep(Duration::from_millis(200)).await;
        })
        .await;

        if drain_result.is_err() {
            tracing::warn!(
                timeout_secs = shutdown_timeout.as_secs(),
                "batch writer drain timed out — some events may be lost"
            );
        } else {
            tracing::info!("batch writer drained successfully");
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
