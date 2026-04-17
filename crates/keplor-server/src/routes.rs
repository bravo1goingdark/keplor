//! HTTP route handlers.

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use metrics_exporter_prometheus::PrometheusHandle;
use serde::{Deserialize, Serialize};
use smol_str::SmolStr;

use keplor_store::{Cursor, EventFilter};

use crate::pipeline::Pipeline;
use crate::schema::{BatchItemResult, BatchRequest, BatchResponse, IngestEvent, IngestResponse};

/// Shared application state.
#[derive(Clone)]
pub struct AppState {
    /// The ingestion pipeline.
    pub pipeline: Pipeline,
    /// Prometheus metrics handle for rendering.
    pub metrics_handle: Arc<PrometheusHandle>,
}

/// `POST /v1/events` — ingest a single event.
pub async fn ingest_single(
    State(state): State<AppState>,
    Json(event): Json<IngestEvent>,
) -> Result<(StatusCode, Json<IngestResponse>), impl IntoResponse> {
    state.pipeline.ingest(event).await.map(|resp| (StatusCode::CREATED, Json(resp)))
}

/// `POST /v1/events/batch` — ingest a batch of events.
///
/// Uses fire-and-forget writes: events are validated and queued for
/// batched storage without awaiting individual flush confirmations.
pub async fn ingest_batch(
    State(state): State<AppState>,
    Json(batch): Json<BatchRequest>,
) -> (StatusCode, Json<BatchResponse>) {
    let mut results = Vec::with_capacity(batch.events.len());
    let mut accepted = 0usize;
    let mut rejected = 0usize;

    for event in batch.events {
        match state.pipeline.ingest_fire_and_forget(event) {
            Ok(resp) => {
                accepted += 1;
                results.push(BatchItemResult::Ok(resp));
            },
            Err(e) => {
                rejected += 1;
                results.push(BatchItemResult::Err { error: e.to_string() });
            },
        }
    }

    let status = if rejected == 0 { StatusCode::CREATED } else { StatusCode::MULTI_STATUS };
    (status, Json(BatchResponse { results, accepted, rejected }))
}

// ── Query API ───────────────────────────────────────────────────────────

/// Query parameters for `GET /v1/events`.
#[derive(Debug, Default, Deserialize)]
pub struct EventQuery {
    /// Filter by user id.
    pub user_id: Option<String>,
    /// Filter by API key id.
    pub api_key_id: Option<String>,
    /// Filter by model name.
    pub model: Option<String>,
    /// Filter by provider.
    pub provider: Option<String>,
    /// Filter by ingestion source.
    pub source: Option<String>,
    /// Events on or after this epoch nanosecond timestamp.
    pub from: Option<i64>,
    /// Events on or before this epoch nanosecond timestamp.
    pub to: Option<i64>,
    /// Maximum results (default 50, max 1000).
    pub limit: Option<u32>,
    /// Cursor for pagination (ts_ns of last item from previous page).
    pub cursor: Option<i64>,
}

/// A single event in the query response (JSON-serialisable).
#[derive(Debug, Serialize)]
pub struct EventResponse {
    pub id: String,
    pub timestamp: i64,
    pub model: String,
    pub provider: String,
    pub usage: UsageResponse,
    pub cost_nanodollars: i64,
    pub latency_total_ms: u32,
    pub latency_ttft_ms: Option<u32>,
    pub http_status: Option<u16>,
    pub source: Option<String>,
    pub user_id: Option<String>,
    pub endpoint: String,
    pub streaming: bool,
}

/// Token usage in query response.
#[derive(Debug, Serialize)]
pub struct UsageResponse {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cache_read_input_tokens: u32,
    pub reasoning_tokens: u32,
}

/// Paginated response for `GET /v1/events`.
#[derive(Debug, Serialize)]
pub struct EventListResponse {
    pub events: Vec<EventResponse>,
    pub cursor: Option<i64>,
    pub has_more: bool,
}

/// `GET /v1/events` — query stored events with filtering and pagination.
pub async fn query_events(
    State(state): State<AppState>,
    Query(params): Query<EventQuery>,
) -> Result<Json<EventListResponse>, crate::error::ServerError> {
    let limit = params.limit.unwrap_or(50).min(1000);

    let filter = EventFilter {
        user_id: params.user_id.map(SmolStr::new),
        api_key_id: params.api_key_id.map(SmolStr::new),
        model: params.model.map(SmolStr::new),
        provider: params.provider.map(SmolStr::new),
        source: params.source.map(SmolStr::new),
        from_ts_ns: params.from,
        to_ts_ns: params.to,
    };

    let cursor = params.cursor.map(Cursor);

    // Run the blocking SQLite query on a dedicated thread to avoid
    // starving tokio worker threads under concurrent load.
    let store = state.pipeline.store_arc();
    let events = tokio::task::spawn_blocking(move || store.query(&filter, limit + 1, cursor))
        .await
        .map_err(|e| crate::error::ServerError::Internal(e.to_string()))?
        .map_err(crate::error::ServerError::from)?;

    let has_more = events.len() > limit as usize;
    let page: Vec<_> = events.into_iter().take(limit as usize).collect();
    let next_cursor = page.last().map(|e| e.ts_ns);

    let responses: Vec<EventResponse> = page
        .into_iter()
        .map(|e| {
            use keplor_core::EventFlags;
            EventResponse {
                id: e.id.to_string(),
                timestamp: e.ts_ns,
                model: e.model.to_string(),
                provider: e.provider.id_key().to_owned(),
                usage: UsageResponse {
                    input_tokens: e.usage.input_tokens,
                    output_tokens: e.usage.output_tokens,
                    cache_read_input_tokens: e.usage.cache_read_input_tokens,
                    reasoning_tokens: e.usage.reasoning_tokens,
                },
                cost_nanodollars: e.cost_nanodollars,
                latency_total_ms: e.latency.total_ms,
                latency_ttft_ms: e.latency.ttft_ms,
                http_status: e.http_status,
                source: e.source.map(|s| s.to_string()),
                user_id: e.user_id.map(|u| u.as_str().to_owned()),
                endpoint: e.endpoint.to_string(),
                streaming: e.flags.contains(EventFlags::STREAMING),
            }
        })
        .collect();

    Ok(Json(EventListResponse {
        events: responses,
        cursor: if has_more { next_cursor } else { None },
        has_more,
    }))
}

// ── Health & Metrics ────────────────────────────────────────────────────

/// `GET /health` — liveness probe with DB check.
pub async fn health(State(state): State<AppState>) -> impl IntoResponse {
    let store = state.pipeline.store_arc();
    let db_ok =
        tokio::task::spawn_blocking(move || store.blob_count().is_ok()).await.unwrap_or(false);
    let status = if db_ok { StatusCode::OK } else { StatusCode::SERVICE_UNAVAILABLE };
    (
        status,
        Json(serde_json::json!({
            "status": if db_ok { "ok" } else { "degraded" },
            "version": env!("CARGO_PKG_VERSION"),
            "db": if db_ok { "connected" } else { "unavailable" },
        })),
    )
}

/// `GET /metrics` — Prometheus text format.
pub async fn metrics_handler(State(state): State<AppState>) -> impl IntoResponse {
    let rendered = state.metrics_handle.render();
    (
        StatusCode::OK,
        [(http::header::CONTENT_TYPE, "text/plain; version=0.0.4; charset=utf-8")],
        rendered,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batch_response_serializes() {
        let resp = BatchResponse {
            results: vec![
                BatchItemResult::Ok(IngestResponse {
                    id: "01ABC".into(),
                    cost_nanodollars: 100,
                    model: "gpt-4o".into(),
                    provider: "openai".into(),
                }),
                BatchItemResult::Err { error: "bad model".into() },
            ],
            accepted: 1,
            rejected: 1,
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("01ABC"));
        assert!(json.contains("bad model"));
    }

    #[test]
    fn event_query_defaults() {
        let q = EventQuery::default();
        assert_eq!(q.limit, None);
        assert!(q.user_id.is_none());
    }
}
