//! HTTP server setup and lifecycle.

use std::net::SocketAddr;
use std::sync::Arc;

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
use crate::routes::{self, AppState};

/// The Keplor ingestion server.
pub struct PipelineServer {
    router: Router,
    addr: SocketAddr,
}

impl PipelineServer {
    /// Build a new server from a pipeline, config, and Prometheus handle.
    pub fn new(
        pipeline: Pipeline,
        keys: ApiKeySet,
        config: &ServerConfig,
        metrics_handle: PrometheusHandle,
    ) -> Self {
        let state = AppState { pipeline, metrics_handle: Arc::new(metrics_handle) };
        let keys = Arc::new(keys);
        let body_limit = config.pipeline.max_body_bytes;

        let authed = Router::new()
            .route("/v1/events", post(routes::ingest_single))
            .route("/v1/events/batch", post(routes::ingest_batch))
            .route("/v1/events", get(routes::query_events))
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

        Self { router, addr: config.server.listen_addr }
    }

    /// Start the server and block until shutdown.
    pub async fn run(self) -> Result<(), std::io::Error> {
        let listener = TcpListener::bind(self.addr).await?;
        tracing::info!(addr = %self.addr, "keplor server listening");

        axum::serve(listener, self.router).with_graceful_shutdown(shutdown_signal()).await
    }

    /// Start the server on an already-bound listener (for tests).
    pub async fn run_on(self, listener: TcpListener) -> Result<(), std::io::Error> {
        let addr = listener.local_addr()?;
        tracing::info!(addr = %addr, "keplor server listening");

        axum::serve(listener, self.router).with_graceful_shutdown(shutdown_signal()).await
    }
}

async fn shutdown_signal() {
    let ctrl_c = tokio::signal::ctrl_c();
    tokio::pin!(ctrl_c);
    ctrl_c.await.ok();
    tracing::info!("shutdown signal received");
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
