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

/// Maximum events per batch request.
const MAX_BATCH_SIZE: usize = 10_000;

/// `POST /v1/events/batch` — ingest a batch of events.
///
/// Uses fire-and-forget writes: events are validated and queued for
/// batched storage without awaiting individual flush confirmations.
/// Events may be lost if the server crashes before the next flush.
pub async fn ingest_batch(
    State(state): State<AppState>,
    Json(batch): Json<BatchRequest>,
) -> Result<(StatusCode, Json<BatchResponse>), crate::error::ServerError> {
    if batch.events.len() > MAX_BATCH_SIZE {
        return Err(crate::error::ServerError::Validation(format!(
            "batch size {} exceeds maximum {MAX_BATCH_SIZE}",
            batch.events.len()
        )));
    }

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
    Ok((status, Json(BatchResponse { results, accepted, rejected })))
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
    /// Only events with http_status >= this value.
    pub status_min: Option<u16>,
    /// Only events with http_status < this value.
    pub status_max: Option<u16>,
    /// Filter by metadata user_tag value.
    pub user_tag: Option<String>,
    /// Filter by metadata session_tag value.
    pub session_tag: Option<String>,
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
    pub api_key_id: Option<String>,
    pub endpoint: String,
    pub streaming: bool,
    pub error: Option<String>,
    pub metadata: Option<serde_json::Value>,
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
        http_status_min: params.status_min,
        http_status_max: params.status_max,
        meta_user_tag: params.user_tag.map(SmolStr::new),
        meta_session_tag: params.session_tag.map(SmolStr::new),
    };

    let cursor = params.cursor.map(Cursor);

    // Use the narrow query path — reads only the 19 columns the API
    // needs instead of all 43 columns.
    let store = state.pipeline.store_arc();
    let events =
        tokio::task::spawn_blocking(move || store.query_summary(&filter, limit + 1, cursor))
            .await
            .map_err(|e| crate::error::ServerError::Internal(e.to_string()))?
            .map_err(crate::error::ServerError::from)?;

    let has_more = events.len() > limit as usize;
    let page: Vec<_> = events.into_iter().take(limit as usize).collect();
    let next_cursor = page.last().map(|e| e.ts_ns);

    let responses: Vec<EventResponse> = page
        .into_iter()
        .map(|e| {
            let metadata = e.metadata_json.as_deref().and_then(|s| serde_json::from_str(s).ok());
            EventResponse {
                id: e.id.to_string(),
                timestamp: e.ts_ns,
                model: e.model,
                provider: e.provider,
                usage: UsageResponse {
                    input_tokens: e.input_tokens,
                    output_tokens: e.output_tokens,
                    cache_read_input_tokens: e.cache_read_input_tokens,
                    reasoning_tokens: e.reasoning_tokens,
                },
                cost_nanodollars: e.cost_nanodollars,
                latency_total_ms: e.total_ms,
                latency_ttft_ms: e.ttft_ms,
                http_status: e.http_status,
                source: e.source,
                user_id: e.user_id,
                api_key_id: e.api_key_id,
                endpoint: e.endpoint,
                streaming: e.streaming,
                error: e.error_type,
                metadata,
            }
        })
        .collect();

    Ok(Json(EventListResponse {
        events: responses,
        cursor: if has_more { next_cursor } else { None },
        has_more,
    }))
}

// ── Aggregation API ────────────────────────────────────────────────────

/// Query parameters for `GET /v1/quota`.
#[derive(Debug, Deserialize)]
pub struct QuotaQuery {
    /// Filter by user id.
    pub user_id: Option<String>,
    /// Filter by API key id.
    pub api_key_id: Option<String>,
    /// Events on or after this epoch nanosecond timestamp (required).
    pub from: i64,
}

/// Response for `GET /v1/quota`.
#[derive(Debug, Serialize)]
pub struct QuotaResponse {
    /// Total cost in nanodollars.
    pub cost_nanodollars: i64,
    /// Number of matching events.
    pub event_count: i64,
}

/// `GET /v1/quota` — real-time cost + event count from `llm_events`.
///
/// At least one of `user_id` or `api_key_id` must be provided to prevent
/// unfiltered scans of the entire table.
pub async fn query_quota(
    State(state): State<AppState>,
    Query(params): Query<QuotaQuery>,
) -> Result<Json<QuotaResponse>, crate::error::ServerError> {
    if params.user_id.is_none() && params.api_key_id.is_none() {
        return Err(crate::error::ServerError::Validation(
            "at least one of user_id or api_key_id is required".into(),
        ));
    }

    let store = state.pipeline.store_arc();
    let summary = tokio::task::spawn_blocking(move || {
        store.quota_summary(params.user_id.as_deref(), params.api_key_id.as_deref(), params.from)
    })
    .await
    .map_err(|e| crate::error::ServerError::Internal(e.to_string()))?
    .map_err(crate::error::ServerError::from)?;

    Ok(Json(QuotaResponse {
        cost_nanodollars: summary.cost_nanodollars,
        event_count: summary.event_count,
    }))
}

/// Query parameters for `GET /v1/rollups`.
#[derive(Debug, Deserialize)]
pub struct RollupsQuery {
    /// Filter by user id.
    pub user_id: Option<String>,
    /// Filter by API key id.
    pub api_key_id: Option<String>,
    /// Start, epoch nanoseconds (converted to day boundary internally).
    pub from: i64,
    /// End, epoch nanoseconds (converted to day boundary internally).
    pub to: i64,
}

/// A single rollup entry in the response.
#[derive(Debug, Serialize)]
pub struct RollupEntry {
    /// Day boundary as epoch seconds.
    pub day: i64,
    /// User id.
    pub user_id: String,
    /// API key id.
    pub api_key_id: String,
    /// Provider id key.
    pub provider: String,
    /// Model name.
    pub model: String,
    /// Number of events.
    pub event_count: i64,
    /// Number of error events (http_status >= 400).
    pub error_count: i64,
    /// Total input tokens.
    pub input_tokens: i64,
    /// Total output tokens.
    pub output_tokens: i64,
    /// Total cache-read input tokens.
    pub cache_read_input_tokens: i64,
    /// Total cache-creation input tokens.
    pub cache_creation_input_tokens: i64,
    /// Total cost in nanodollars.
    pub cost_nanodollars: i64,
}

/// Response for `GET /v1/rollups`.
#[derive(Debug, Serialize)]
pub struct RollupsResponse {
    /// Daily rollup rows.
    pub rollups: Vec<RollupEntry>,
}

/// `GET /v1/rollups` — pre-aggregated daily rollup rows.
pub async fn query_rollups(
    State(state): State<AppState>,
    Query(params): Query<RollupsQuery>,
) -> Result<Json<RollupsResponse>, crate::error::ServerError> {
    let from_day = ns_to_day_epoch(params.from);
    let to_day = ns_to_day_epoch(params.to);

    let store = state.pipeline.store_arc();
    let rows = tokio::task::spawn_blocking(move || {
        store.query_rollups(
            params.user_id.as_deref(),
            params.api_key_id.as_deref(),
            from_day,
            to_day,
        )
    })
    .await
    .map_err(|e| crate::error::ServerError::Internal(e.to_string()))?
    .map_err(crate::error::ServerError::from)?;

    let rollups = rows
        .into_iter()
        .map(|r| RollupEntry {
            day: r.day,
            user_id: r.user_id,
            api_key_id: r.api_key_id,
            provider: r.provider,
            model: r.model,
            event_count: r.event_count,
            error_count: r.error_count,
            input_tokens: r.input_tokens,
            output_tokens: r.output_tokens,
            cache_read_input_tokens: r.cache_read_input_tokens,
            cache_creation_input_tokens: r.cache_creation_input_tokens,
            cost_nanodollars: r.cost_nanodollars,
        })
        .collect();

    Ok(Json(RollupsResponse { rollups }))
}

/// Query parameters for `GET /v1/stats`.
#[derive(Debug, Deserialize)]
pub struct StatsQuery {
    /// Filter by user id.
    pub user_id: Option<String>,
    /// Filter by API key id.
    pub api_key_id: Option<String>,
    /// Start, epoch nanoseconds.
    pub from: i64,
    /// End, epoch nanoseconds.
    pub to: i64,
    /// Optional provider filter.
    pub provider: Option<String>,
    /// Set to `"model"` to group results by provider+model.
    pub group_by: Option<String>,
}

/// A single stats entry in the response.
#[derive(Debug, Serialize)]
pub struct StatEntry {
    /// Provider (empty when not grouped).
    pub provider: String,
    /// Model name (empty when not grouped).
    pub model: String,
    /// Number of events.
    pub event_count: i64,
    /// Number of error events.
    pub error_count: i64,
    /// Total input tokens.
    pub input_tokens: i64,
    /// Total output tokens.
    pub output_tokens: i64,
    /// Total cache-read input tokens.
    pub cache_read_input_tokens: i64,
    /// Total cache-creation input tokens.
    pub cache_creation_input_tokens: i64,
    /// Total cost in nanodollars.
    pub cost_nanodollars: i64,
}

/// Response for `GET /v1/stats`.
#[derive(Debug, Serialize)]
pub struct StatsResponse {
    /// Aggregated stat rows.
    pub stats: Vec<StatEntry>,
}

/// `GET /v1/stats` — aggregated period statistics from `daily_rollups`.
pub async fn query_stats(
    State(state): State<AppState>,
    Query(params): Query<StatsQuery>,
) -> Result<Json<StatsResponse>, crate::error::ServerError> {
    let from_day = ns_to_day_epoch(params.from);
    let to_day = ns_to_day_epoch(params.to);
    let group_by_model = params.group_by.as_deref() == Some("model");

    let store = state.pipeline.store_arc();
    let rows = tokio::task::spawn_blocking(move || {
        store.aggregate_stats(
            params.user_id.as_deref(),
            params.api_key_id.as_deref(),
            from_day,
            to_day,
            params.provider.as_deref(),
            group_by_model,
        )
    })
    .await
    .map_err(|e| crate::error::ServerError::Internal(e.to_string()))?
    .map_err(crate::error::ServerError::from)?;

    let stats = rows
        .into_iter()
        .map(|r| StatEntry {
            provider: r.provider,
            model: r.model,
            event_count: r.event_count,
            error_count: r.error_count,
            input_tokens: r.input_tokens,
            output_tokens: r.output_tokens,
            cache_read_input_tokens: r.cache_read_input_tokens,
            cache_creation_input_tokens: r.cache_creation_input_tokens,
            cost_nanodollars: r.cost_nanodollars,
        })
        .collect();

    Ok(Json(StatsResponse { stats }))
}

/// Convert epoch nanoseconds to a day boundary in epoch seconds.
fn ns_to_day_epoch(ns: i64) -> i64 {
    let secs = ns / 1_000_000_000;
    secs - (secs % 86400)
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
