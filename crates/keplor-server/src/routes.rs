//! HTTP route handlers.

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{Extension, Json};
use metrics_exporter_prometheus::PrometheusHandle;
use serde::{Deserialize, Serialize};
use smol_str::SmolStr;

use keplor_store::{Cursor, EventFilter};

use crate::auth::AuthenticatedKey;
use crate::pipeline::Pipeline;
use crate::schema::{BatchItemResult, BatchRequest, BatchResponse, IngestEvent, IngestResponse};

/// Shared application state.
#[derive(Clone)]
pub struct AppState {
    /// The ingestion pipeline.
    pub pipeline: Pipeline,
    /// Prometheus metrics handle for rendering.
    pub metrics_handle: Arc<PrometheusHandle>,
    /// Default retention tier for unauthenticated requests.
    pub default_tier: SmolStr,
    /// Hot-swappable handle to the S3 archiver. `run()` populates
    /// this once `[archive]` connectivity is verified; until then
    /// `?include_archived=true` falls through to live-only.
    #[cfg(feature = "s3")]
    pub archiver: Arc<arc_swap::ArcSwap<Option<Arc<keplor_store::Archiver>>>>,
}

/// Query parameters accepted by `POST /v1/events`.
#[derive(Debug, Deserialize, Default)]
pub struct IngestQuery {
    /// `false` skips the await-flush oneshot — the request returns
    /// `202 Accepted` as soon as the event is enqueued. Default
    /// `true` (durable; returns `201 Created` after flush). Use
    /// `?durable=false` for hot-path traffic that doesn't need
    /// per-event flush confirmation; events may be lost if the
    /// process crashes before the next batch flush (~50 ms).
    #[serde(default = "default_true")]
    pub durable: bool,
}

fn default_true() -> bool {
    true
}

/// `POST /v1/events` — ingest a single event.
///
/// Two modes:
/// - **Durable (default)**: awaits the BatchWriter flush; returns
///   `201 Created` once the event has hit segment files. Bounded by
///   `pipeline.flush_interval_ms` worst-case latency.
/// - **Fire-and-forget** (`?durable=false`): enqueues + returns
///   `202 Accepted` immediately. ~1000× lower per-request latency,
///   no acknowledgement of disk durability.
pub async fn ingest_single(
    State(state): State<AppState>,
    auth: Option<Extension<AuthenticatedKey>>,
    headers: axum::http::HeaderMap,
    Query(query): Query<IngestQuery>,
    Json(event): Json<IngestEvent>,
) -> Result<(StatusCode, Json<IngestResponse>), impl IntoResponse> {
    let (key_id, tier) = auth
        .map(|Extension(k)| (Some(k.key_id), k.tier.to_string()))
        .unwrap_or((None, state.default_tier.to_string()));
    let idempotency_key =
        headers.get("idempotency-key").and_then(|v| v.to_str().ok()).map(String::from);

    if query.durable {
        state
            .pipeline
            .ingest(event, key_id.as_deref(), idempotency_key.as_deref(), &tier)
            .await
            .map(|resp| (StatusCode::CREATED, Json(resp)))
    } else {
        // Fire-and-forget: enqueue without awaiting the flush oneshot.
        // The pipeline still does validate → normalize → cost → build
        // LlmEvent synchronously, then non-blocking enqueue.
        state
            .pipeline
            .ingest_fire_and_forget(event, key_id.as_deref(), &tier)
            .map(|resp| (StatusCode::ACCEPTED, Json(resp)))
    }
}

/// Maximum events per batch request.
const MAX_BATCH_SIZE: usize = 10_000;

/// `POST /v1/events/batch` — ingest a batch of events.
///
/// By default, uses fire-and-forget writes: events are validated and
/// queued for batched storage without awaiting individual flush
/// confirmations. Events may be lost if the server crashes before the
/// next flush (~50 ms).
///
/// Set the `X-Keplor-Durable: true` header to await each write's flush
/// confirmation before responding — slower, but every accepted event is
/// guaranteed durable when the response arrives.
pub async fn ingest_batch(
    State(state): State<AppState>,
    auth: Option<Extension<AuthenticatedKey>>,
    headers: axum::http::HeaderMap,
    Json(batch): Json<BatchRequest>,
) -> Result<(StatusCode, Json<BatchResponse>), crate::error::ServerError> {
    if batch.events.len() > MAX_BATCH_SIZE {
        return Err(crate::error::ServerError::Validation(format!(
            "batch size {} exceeds maximum {MAX_BATCH_SIZE}",
            batch.events.len()
        )));
    }

    let durable = headers
        .get("x-keplor-durable")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.eq_ignore_ascii_case("true"));

    let (key_id, tier) = auth
        .map(|Extension(k)| (Some(k.key_id), k.tier.to_string()))
        .unwrap_or((None, state.default_tier.to_string()));
    let mut results = Vec::with_capacity(batch.events.len());
    let mut accepted = 0usize;
    let mut rejected = 0usize;

    if durable {
        let batch_results =
            state.pipeline.ingest_batch_durable(batch.events, key_id.as_deref(), &tier).await;
        for result in batch_results {
            match result {
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
    } else {
        for event in batch.events {
            match state.pipeline.ingest_fire_and_forget(event, key_id.as_deref(), &tier) {
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
    /// Merge archived (S3) events into the response. Default `false`
    /// (live-only). When `true`, the server fetches every archive
    /// manifest overlapping `[from, to]` for the requested user (if
    /// any), decompresses + parses the JSONL chunks, applies the
    /// same filter, and merges the result with live events sorted
    /// `(ts_ns desc, id desc)`. Currently uncached — each request
    /// pays the round-trip cost; suitable for backfill / audit, not
    /// hot dashboards.
    #[serde(default)]
    pub include_archived: bool,
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
    pub cache_creation_input_tokens: u32,
    pub reasoning_tokens: u32,
}

/// Paginated response for `GET /v1/events`.
#[derive(Debug, Serialize)]
pub struct EventListResponse {
    pub events: Vec<EventResponse>,
    pub cursor: Option<i64>,
    pub has_more: bool,
    /// Whether archived (S3) data exists for the queried time range.
    pub has_archived_data: bool,
}

/// `GET /v1/events` — query stored events with filtering and pagination.
pub async fn query_events(
    State(state): State<AppState>,
    Query(params): Query<EventQuery>,
) -> Result<Json<EventListResponse>, crate::error::ServerError> {
    let limit = params.limit.unwrap_or(50).min(1000);

    let filter = EventFilter {
        user_id: params.user_id.clone().map(SmolStr::new),
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
    //
    // `spawn_blocking` defers to a blocking-thread pool that has no
    // active tracing span, so `Span::current()` inside the closure
    // would otherwise be empty and `#[tracing::instrument]` on the
    // store method would create a *root* span detached from
    // `request_id`. Capture the current span and re-enter it in the
    // closure to keep the trace contiguous.
    let store = state.pipeline.store_arc();
    let span = tracing::Span::current();
    let events = tokio::task::spawn_blocking(move || {
        span.in_scope(|| store.query_summary(&filter, limit + 1, cursor))
    })
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
                    cache_creation_input_tokens: e.cache_creation_input_tokens,
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

    // Check if archived data exists for this time range.
    let from_ts = params.from;
    let to_ts = params.to;
    let archive_store = state.pipeline.store_arc();
    let archive_span = tracing::Span::current();
    let has_archived_data = tokio::task::spawn_blocking(move || {
        archive_span.in_scope(|| archive_store.has_archived_data(from_ts, to_ts).unwrap_or(false))
    })
    .await
    .unwrap_or(false);

    // Optional transparent merge of archived events. Opt-in via
    // ?include_archived=true. Implementation is uncached: each
    // request pays the per-manifest round-trip + decompress.
    #[cfg(feature = "s3")]
    let responses = if params.include_archived {
        merge_archived_events(
            responses,
            &state,
            params.user_id.as_deref(),
            params.from,
            params.to,
            limit as usize,
        )
        .await
    } else {
        responses
    };

    let archived_has_more = false; // Reserved for follow-up cursor work.
    let _ = archived_has_more;

    Ok(Json(EventListResponse {
        events: responses,
        cursor: if has_more { next_cursor } else { None },
        has_more,
        has_archived_data,
    }))
}

/// Merge archived events into the live result.
///
/// Archived chunks come from S3 — we fetch every manifest whose
/// `[min_ts_ns, max_ts_ns]` overlaps the request window and whose
/// `user_id` matches the filter (if any). The fetched events are
/// converted to `EventResponse` and concatenated with the live
/// result, sorted `(ts_ns desc, id desc)`, then truncated to `limit`.
///
/// This is a one-shot best-effort merge: failures fetching individual
/// chunks emit a warn and are skipped, so a single 5xx from S3
/// doesn't cripple the whole query. Pagination across the
/// archive/live boundary is **not** implemented yet — callers asking
/// for archived data should request small windows.
#[cfg(feature = "s3")]
async fn merge_archived_events(
    mut live: Vec<EventResponse>,
    state: &AppState,
    user_filter: Option<&str>,
    from_ts: Option<i64>,
    to_ts: Option<i64>,
    limit: usize,
) -> Vec<EventResponse> {
    let archiver_snapshot = state.archiver.load();
    let archiver = match archiver_snapshot.as_ref() {
        Some(a) => Arc::clone(a),
        None => {
            // s3 feature compiled in but no archiver configured —
            // nothing to merge.
            return live;
        },
    };

    // List manifests that overlap [from, to] for this tenant. Tenant
    // filter is applied at this stage so a request without
    // `user_id` (admin / cross-tenant query) still works, but a
    // user-scoped query never fetches another user's S3 keys.
    let store = state.pipeline.store_arc();
    let user_owned = user_filter.map(|s| s.to_owned());
    let manifests = match tokio::task::spawn_blocking(move || {
        store.list_archives(user_owned.as_deref(), from_ts, to_ts)
    })
    .await
    {
        Ok(Ok(m)) => m,
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "list_archives failed; returning live-only");
            return live;
        },
        Err(e) => {
            tracing::warn!(error = %e, "list_archives task panicked; returning live-only");
            return live;
        },
    };

    if manifests.is_empty() {
        return live;
    }

    let fetch_started = std::time::Instant::now();
    let mut archived_events = Vec::new();
    for m in &manifests {
        match archiver.fetch_one(m).await {
            Ok(events) => archived_events.extend(events),
            Err(e) => {
                tracing::warn!(
                    archive_id = %m.archive_id,
                    error = %e,
                    "archive chunk fetch failed; skipping",
                );
                metrics::counter!("keplor_archive_fetch_errors_total").increment(1);
            },
        }
    }
    metrics::histogram!("keplor_archive_fetch_seconds")
        .record(fetch_started.elapsed().as_secs_f64());

    // Convert archived events to EventResponse, applying the same
    // server-side filters as live events. The query parameters that
    // matter for the in-memory filter are user_id (already applied
    // via the manifest tenant filter) and the time range (which we
    // re-check here in case a manifest's [min,max] overlaps but
    // individual events fall outside).
    let from = from_ts.unwrap_or(i64::MIN);
    let to = to_ts.unwrap_or(i64::MAX);
    for ev in archived_events {
        if ev.ts_ns < from || ev.ts_ns > to {
            continue;
        }
        if let Some(u) = user_filter {
            if ev.user_id.as_ref().map(|s| s.as_str()) != Some(u) {
                continue;
            }
        }
        live.push(llm_event_to_response(ev));
    }

    let mut merged = live;
    merged.sort_by(|a, b| match b.timestamp.cmp(&a.timestamp) {
        std::cmp::Ordering::Equal => b.id.cmp(&a.id),
        other => other,
    });
    merged.truncate(limit);
    merged
}

#[cfg(feature = "s3")]
fn llm_event_to_response(ev: keplor_core::LlmEvent) -> EventResponse {
    EventResponse {
        id: ev.id.to_string(),
        timestamp: ev.ts_ns,
        model: ev.model.to_string(),
        provider: ev.provider.id_key().to_owned(),
        usage: UsageResponse {
            input_tokens: ev.usage.input_tokens,
            output_tokens: ev.usage.output_tokens,
            cache_read_input_tokens: ev.usage.cache_read_input_tokens,
            cache_creation_input_tokens: ev.usage.cache_creation_input_tokens,
            reasoning_tokens: ev.usage.reasoning_tokens,
        },
        cost_nanodollars: ev.cost_nanodollars,
        latency_total_ms: ev.latency.total_ms,
        latency_ttft_ms: ev.latency.ttft_ms,
        http_status: ev.http_status,
        source: ev.source.map(|s| s.to_string()),
        user_id: ev.user_id.map(|u| u.as_str().to_owned()),
        api_key_id: ev.api_key_id.map(|k| k.as_str().to_owned()),
        endpoint: ev.endpoint.to_string(),
        streaming: ev.flags.contains(keplor_core::EventFlags::STREAMING),
        error: ev.error.as_ref().map(|e| {
            match e {
                keplor_core::ProviderError::RateLimited { .. } => "rate_limited",
                keplor_core::ProviderError::InvalidRequest(_) => "invalid_request",
                keplor_core::ProviderError::AuthFailed => "auth_failed",
                keplor_core::ProviderError::ContextLengthExceeded { .. } => {
                    "context_length_exceeded"
                },
                keplor_core::ProviderError::ContentFiltered { .. } => "content_filtered",
                keplor_core::ProviderError::UpstreamTimeout => "upstream_timeout",
                keplor_core::ProviderError::UpstreamUnavailable => "upstream_unavailable",
                keplor_core::ProviderError::Other { .. } => "other",
            }
            .to_owned()
        }),
        metadata: ev.metadata,
    }
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
    let span = tracing::Span::current();
    let summary = tokio::task::spawn_blocking(move || {
        span.in_scope(|| {
            store.quota_summary(
                params.user_id.as_deref(),
                params.api_key_id.as_deref(),
                params.from,
            )
        })
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
    /// Maximum results (default 100, max 1000).
    pub limit: Option<u32>,
    /// Offset for pagination (default 0).
    pub offset: Option<u32>,
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
    /// Whether more rows exist beyond this page.
    pub has_more: bool,
}

/// `GET /v1/rollups` — pre-aggregated daily rollup rows.
pub async fn query_rollups(
    State(state): State<AppState>,
    Query(params): Query<RollupsQuery>,
) -> Result<Json<RollupsResponse>, crate::error::ServerError> {
    let from_day = ns_to_day_epoch(params.from);
    let to_day = ns_to_day_epoch(params.to);
    let limit = params.limit.unwrap_or(100).min(1000);
    let offset = params.offset.unwrap_or(0);

    let store = state.pipeline.store_arc();
    let span = tracing::Span::current();
    let rows = tokio::task::spawn_blocking(move || {
        span.in_scope(|| {
            store.query_rollups(
                params.user_id.as_deref(),
                params.api_key_id.as_deref(),
                from_day,
                to_day,
                limit + 1,
                offset,
            )
        })
    })
    .await
    .map_err(|e| crate::error::ServerError::Internal(e.to_string()))?
    .map_err(crate::error::ServerError::from)?;

    let has_more = rows.len() > limit as usize;

    let rollups: Vec<_> = rows
        .into_iter()
        .take(limit as usize)
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

    Ok(Json(RollupsResponse { rollups, has_more }))
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
    /// Maximum results (default 100, max 1000).
    pub limit: Option<u32>,
    /// Offset for pagination (default 0).
    pub offset: Option<u32>,
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
    /// Whether more rows exist beyond this page.
    pub has_more: bool,
}

/// `GET /v1/stats` — aggregated period statistics from `daily_rollups`.
pub async fn query_stats(
    State(state): State<AppState>,
    Query(params): Query<StatsQuery>,
) -> Result<Json<StatsResponse>, crate::error::ServerError> {
    let from_day = ns_to_day_epoch(params.from);
    let to_day = ns_to_day_epoch(params.to);
    let group_by_model = params.group_by.as_deref() == Some("model");
    let limit = params.limit.unwrap_or(100).min(1000);
    let offset = params.offset.unwrap_or(0);

    let store = state.pipeline.store_arc();
    let span = tracing::Span::current();
    let rows = tokio::task::spawn_blocking(move || {
        span.in_scope(|| {
            store.aggregate_stats(
                params.user_id.as_deref(),
                params.api_key_id.as_deref(),
                from_day,
                to_day,
                params.provider.as_deref(),
                group_by_model,
                limit + 1,
                offset,
            )
        })
    })
    .await
    .map_err(|e| crate::error::ServerError::Internal(e.to_string()))?
    .map_err(crate::error::ServerError::from)?;

    let has_more = rows.len() > limit as usize;
    let stats: Vec<_> = rows
        .into_iter()
        .take(limit as usize)
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

    Ok(Json(StatsResponse { stats, has_more }))
}

/// Convert epoch nanoseconds to a day boundary in epoch seconds.
fn ns_to_day_epoch(ns: i64) -> i64 {
    let secs = ns / 1_000_000_000;
    secs - (secs % 86400)
}

// ── Deletion API ───────────────────────────────────────────────────────

/// `DELETE /v1/events/:id` — delete a single event by ID.
pub async fn delete_event(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<StatusCode, crate::error::ServerError> {
    let event_id: keplor_core::EventId =
        id.parse().map_err(|_| crate::error::ServerError::Validation("invalid event id".into()))?;

    let store = state.pipeline.store_arc();
    let span = tracing::Span::current();
    let deleted =
        tokio::task::spawn_blocking(move || span.in_scope(|| store.delete_event(&event_id)))
            .await
            .map_err(|e| crate::error::ServerError::Internal(e.to_string()))?
            .map_err(crate::error::ServerError::from)?;

    if deleted {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Ok(StatusCode::NOT_FOUND)
    }
}

/// Query parameters for `DELETE /v1/events`.
///
/// Exactly ONE of `older_than_days` or `user_id` must be provided.
/// Both forms tombstone the matching events; storage reclamation
/// happens on the next GC sweep.
#[derive(Debug, Deserialize)]
pub struct DeleteEventsQuery {
    /// Delete events older than this many days. Mutually exclusive with `user_id`.
    pub older_than_days: Option<u32>,
    /// Delete every event for this `user_id`. GDPR right-to-erasure entry point.
    /// Mutually exclusive with `older_than_days`.
    pub user_id: Option<String>,
}

/// Response for bulk deletion.
#[derive(Debug, Serialize)]
pub struct DeleteEventsResponse {
    /// Number of events deleted.
    pub events_deleted: usize,
    /// Number of orphaned blobs deleted.
    pub blobs_deleted: usize,
}

/// `DELETE /v1/events?older_than_days=N` — bulk delete old events.
/// `DELETE /v1/events?user_id=alice` — delete every event for one user (GDPR).
pub async fn delete_events_bulk(
    State(state): State<AppState>,
    // Optional extractor: when the server runs without API keys
    // configured (`auth.api_keys = []`), `require_api_key` short-circuits
    // without inserting the extension. Treat None as anonymous and tag
    // the audit log accordingly so dev/local-mode use isn't broken.
    auth: Option<axum::extract::Extension<crate::auth::AuthenticatedKey>>,
    Query(params): Query<DeleteEventsQuery>,
) -> Result<Json<DeleteEventsResponse>, crate::error::ServerError> {
    let actor_key_id =
        auth.as_ref().map(|e| e.0.key_id.to_string()).unwrap_or_else(|| "anon".to_string());
    match (params.older_than_days, params.user_id.as_ref()) {
        (Some(_), Some(_)) => {
            return Err(crate::error::ServerError::Validation(
                "older_than_days and user_id are mutually exclusive".into(),
            ));
        },
        (None, None) => {
            return Err(crate::error::ServerError::Validation(
                "one of older_than_days or user_id is required".into(),
            ));
        },
        _ => {},
    }

    if let Some(days) = params.older_than_days {
        if days == 0 {
            return Err(crate::error::ServerError::Validation(
                "older_than_days must be > 0".into(),
            ));
        }
        let cutoff_ns = {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos() as i64;
            now - (days as i64) * 86_400 * 1_000_000_000
        };
        let store = state.pipeline.store_arc();
        let span = tracing::Span::current();
        let stats =
            tokio::task::spawn_blocking(move || span.in_scope(|| store.gc_expired(cutoff_ns)))
                .await
                .map_err(|e| crate::error::ServerError::Internal(e.to_string()))?
                .map_err(crate::error::ServerError::from)?;

        // Audit log: who triggered the bulk delete and what window.
        tracing::warn!(
            target: "audit",
            actor_key_id = %actor_key_id,
            mode = "older_than_days",
            older_than_days = days,
            events_deleted = stats.events_deleted,
            "bulk delete: time-window"
        );

        return Ok(Json(DeleteEventsResponse {
            events_deleted: stats.events_deleted,
            blobs_deleted: stats.blobs_deleted,
        }));
    }

    // user_id path — GDPR right-to-erasure.  The match above already
    // returns on both (None, None) and (Some, Some); reaching here
    // implies user_id is Some, but route the impossible None through a
    // typed error so clippy::expect_used stays happy and a future
    // refactor can't accidentally break the invariant silently.
    let Some(user_id) = params.user_id else {
        return Err(crate::error::ServerError::Internal(
            "user_id missing after validation — should be unreachable".into(),
        ));
    };
    if user_id.trim().is_empty() {
        return Err(crate::error::ServerError::Validation("user_id must not be empty".into()));
    }

    let store = state.pipeline.store_arc();
    let user = SmolStr::new(&user_id);
    let span = tracing::Span::current();

    // Loop in bounded batches: query → collect IDs → delete → repeat.
    // We intentionally don't load the entire user history into memory
    // for high-volume deletions.
    const BATCH: u32 = 1000;
    let mut total_deleted: usize = 0;
    let actor_key = actor_key_id.clone();
    loop {
        let store_q = Arc::clone(&store);
        let user_q = user.clone();
        let span_q = span.clone();
        let ids = tokio::task::spawn_blocking(move || {
            span_q.in_scope(|| {
                let filter =
                    keplor_store::EventFilter { user_id: Some(user_q), ..Default::default() };
                store_q.query(&filter, BATCH, None)
            })
        })
        .await
        .map_err(|e| crate::error::ServerError::Internal(e.to_string()))?
        .map_err(crate::error::ServerError::from)?
        .into_iter()
        .map(|ev| ev.id)
        .collect::<Vec<_>>();
        if ids.is_empty() {
            break;
        }
        let store_d = Arc::clone(&store);
        let span_d = span.clone();
        let n = ids.len();
        let deleted = tokio::task::spawn_blocking(move || {
            span_d.in_scope(|| store_d.delete_events_by_ids(&ids))
        })
        .await
        .map_err(|e| crate::error::ServerError::Internal(e.to_string()))?
        .map_err(crate::error::ServerError::from)?;
        total_deleted += deleted;
        // The query returns same events until a delete sweep visibility
        // catches up. If the batch returned the same events twice,
        // we'd loop forever. delete returns the count actually
        // tombstoned; if zero (everything was already gone), stop.
        if deleted == 0 || n < BATCH as usize {
            break;
        }
    }

    tracing::warn!(
        target: "audit",
        actor_key_id = %actor_key,
        mode = "user_id",
        user_id = %user_id,
        events_deleted = total_deleted,
        "bulk delete: GDPR by user_id"
    );

    Ok(Json(DeleteEventsResponse { events_deleted: total_deleted, blobs_deleted: 0 }))
}

// ── Export API ──────────────────────────────────────────────────────────

/// `GET /v1/events/export` — stream all matching events as JSON Lines.
pub async fn export_events(
    State(state): State<AppState>,
    Query(params): Query<EventQuery>,
) -> Result<impl IntoResponse, crate::error::ServerError> {
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

    let store = state.pipeline.store_arc();
    let mut lines = Vec::new();
    let span = tracing::Span::current();
    tokio::task::spawn_blocking(move || {
        span.in_scope(|| {
            store
                .export_events(&filter, &mut |event| {
                    let resp = EventResponse {
                        id: event.id.to_string(),
                        timestamp: event.ts_ns,
                        model: event.model,
                        provider: event.provider,
                        usage: UsageResponse {
                            input_tokens: event.input_tokens,
                            output_tokens: event.output_tokens,
                            cache_read_input_tokens: event.cache_read_input_tokens,
                            cache_creation_input_tokens: event.cache_creation_input_tokens,
                            reasoning_tokens: event.reasoning_tokens,
                        },
                        cost_nanodollars: event.cost_nanodollars,
                        latency_total_ms: event.total_ms,
                        latency_ttft_ms: event.ttft_ms,
                        http_status: event.http_status,
                        source: event.source,
                        user_id: event.user_id,
                        api_key_id: event.api_key_id,
                        endpoint: event.endpoint,
                        streaming: event.streaming,
                        error: event.error_type,
                        metadata: event
                            .metadata_json
                            .as_deref()
                            .and_then(|s| serde_json::from_str(s).ok()),
                    };
                    if let Ok(json) = serde_json::to_string(&resp) {
                        lines.push(json);
                    }
                })
                .map(|()| lines)
        })
    })
    .await
    .map_err(|e| crate::error::ServerError::Internal(e.to_string()))?
    .map(|lines| {
        let body = lines.join("\n");
        ([(http::header::CONTENT_TYPE, "application/x-ndjson")], body)
    })
    .map_err(crate::error::ServerError::from)
}

// ── Health & Metrics ────────────────────────────────────────────────────

/// `GET /health` — liveness probe with DB and queue status.
pub async fn health(State(state): State<AppState>) -> impl IntoResponse {
    let store = state.pipeline.store_arc();
    let db_ok =
        tokio::task::spawn_blocking(move || store.health_probe().is_ok()).await.unwrap_or(false);

    let queue_depth = state.pipeline.queue_depth();
    let queue_capacity = state.pipeline.queue_capacity();
    let queue_pct = if queue_capacity > 0 {
        (queue_depth as f64 / queue_capacity as f64 * 100.0) as u32
    } else {
        0
    };

    let status = if db_ok { StatusCode::OK } else { StatusCode::SERVICE_UNAVAILABLE };
    (
        status,
        Json(serde_json::json!({
            "status": if db_ok { "ok" } else { "degraded" },
            "version": env!("CARGO_PKG_VERSION"),
            "db": if db_ok { "connected" } else { "unavailable" },
            "queue_depth": queue_depth,
            "queue_capacity": queue_capacity,
            "queue_utilization_pct": queue_pct,
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
