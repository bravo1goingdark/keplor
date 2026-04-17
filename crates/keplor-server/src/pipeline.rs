//! Ingestion pipeline: validate → normalise → compute cost → store.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use keplor_core::{EventFlags, EventId, Latencies, LlmEvent, Provider, ProviderError, Usage};
use keplor_pricing::compute::{compute_cost, CostOpts};
use keplor_pricing::{Catalog, ModelKey};
use keplor_store::{BatchWriter, Store};
use sha2::{Digest, Sha256};
use smol_str::SmolStr;

use crate::error::ServerError;
use crate::normalize;
use crate::schema::{IngestEvent, IngestResponse, TimestampInput};
use crate::validate;

/// Default cost options — reused across all events to avoid per-call construction.
static DEFAULT_COST_OPTS: CostOpts = CostOpts {
    is_batch: false,
    service_tier: keplor_pricing::compute::ServiceTier::Standard,
    inference_geo: keplor_pricing::compute::InferenceGeo::Us,
    cache_ttl: keplor_pricing::compute::CacheTtl::Minutes5,
    context_bucket: keplor_pricing::compute::ContextBucket::Standard,
};

/// Shared state for the pipeline.
#[derive(Clone)]
pub struct Pipeline {
    store: Arc<Store>,
    writer: Arc<BatchWriter>,
    catalog: Arc<Catalog>,
}

impl Pipeline {
    /// Create a new pipeline with the given store, batch writer, and pricing catalog.
    pub fn new(store: Arc<Store>, writer: Arc<BatchWriter>, catalog: Arc<Catalog>) -> Self {
        Self { store, writer, catalog }
    }

    /// Process a single ingestion event — durable write (awaits flush).
    ///
    /// Times out after 10 seconds to prevent indefinite hangs if the
    /// batch writer stalls.
    pub async fn ingest(&self, event: IngestEvent) -> Result<IngestResponse, ServerError> {
        let (llm_event, req_bytes, resp_bytes, provider, model, cost) =
            self.process_event(event)?;

        let id = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            self.writer.write(llm_event, req_bytes, resp_bytes),
        )
        .await
        .map_err(|_| ServerError::Internal("write timed out after 10s".into()))?
        .map_err(|e| {
            metrics::counter!("keplor_events_errors_total", "stage" => "store").increment(1);
            ServerError::from(e)
        })?;

        self.emit_metrics(&provider, &model);

        Ok(IngestResponse {
            id: id.to_string(),
            cost_nanodollars: cost,
            model: model.to_string(),
            provider: provider.id_key().to_owned(),
        })
    }

    /// Process and submit without awaiting flush — for batch endpoints.
    ///
    /// Events are queued for batched writes. If the queue is full, returns
    /// an error. Events may be lost if the server crashes before flushing.
    pub fn ingest_fire_and_forget(
        &self,
        event: IngestEvent,
    ) -> Result<IngestResponse, ServerError> {
        let (llm_event, req_bytes, resp_bytes, provider, model, cost) =
            self.process_event(event)?;

        let id = llm_event.id;
        self.writer.write_fire_and_forget(llm_event, req_bytes, resp_bytes).map_err(|e| {
            metrics::counter!("keplor_events_errors_total", "stage" => "queue_full").increment(1);
            ServerError::from(e)
        })?;

        self.emit_metrics(&provider, &model);

        Ok(IngestResponse {
            id: id.to_string(),
            cost_nanodollars: cost,
            model: model.to_string(),
            provider: provider.id_key().to_owned(),
        })
    }

    /// Core processing: validate → normalize → cost → build event → serialize bodies.
    ///
    /// Returns `(LlmEvent, req_bytes, resp_bytes, provider, model, cost)`.
    fn process_event(
        &self,
        event: IngestEvent,
    ) -> Result<(LlmEvent, Bytes, Bytes, Provider, SmolStr, i64), ServerError> {
        validate::validate(&event).inspect_err(|_| {
            metrics::counter!("keplor_events_errors_total", "stage" => "validation").increment(1);
        })?;

        let provider = normalize::normalize_provider(&event.provider);
        let model = normalize::normalize_model(&event.model);

        let usage = usage_from_ingest(&event.usage);
        let cost = self.compute_cost(&provider, &model, &usage);

        // Serialize bodies ONCE — propagate errors instead of silently storing empty blobs.
        let req_bytes = match &event.request_body {
            Some(v) => Bytes::from(
                serde_json::to_vec(v)
                    .map_err(|e| ServerError::Json(format!("request body: {e}")))?,
            ),
            None => Bytes::new(),
        };
        let resp_bytes = match &event.response_body {
            Some(v) => Bytes::from(
                serde_json::to_vec(v)
                    .map_err(|e| ServerError::Json(format!("response body: {e}")))?,
            ),
            None => Bytes::new(),
        };

        let llm_event = build_llm_event(
            &event,
            provider.clone(),
            model.clone(),
            cost,
            usage,
            &req_bytes,
            &resp_bytes,
        )?;

        Ok((llm_event, req_bytes, resp_bytes, provider, model, cost))
    }

    /// Direct store access for queries.
    pub fn store(&self) -> &Store {
        &self.store
    }

    /// Shared store handle for `spawn_blocking` closures.
    pub fn store_arc(&self) -> Arc<Store> {
        Arc::clone(&self.store)
    }

    #[inline]
    fn compute_cost(&self, provider: &Provider, model: &str, usage: &Usage) -> i64 {
        let key = ModelKey::from_normalized(SmolStr::new(model));
        match self.catalog.lookup(&key) {
            Some(p) => compute_cost(provider, p, usage, &DEFAULT_COST_OPTS).nanodollars(),
            None => {
                tracing::warn!(model, "no pricing found, cost = 0");
                0
            },
        }
    }

    #[inline]
    fn emit_metrics(&self, provider: &Provider, _model: &SmolStr) {
        metrics::counter!("keplor_events_ingested_total",
            "provider" => provider.id_key(),
        )
        .increment(1);
    }
}

fn usage_from_ingest(u: &crate::schema::IngestUsage) -> Usage {
    Usage {
        input_tokens: u.input_tokens,
        output_tokens: u.output_tokens,
        cache_read_input_tokens: u.cache_read_input_tokens,
        cache_creation_input_tokens: u.cache_creation_input_tokens,
        reasoning_tokens: u.reasoning_tokens,
        audio_input_tokens: u.audio_input_tokens,
        audio_output_tokens: u.audio_output_tokens,
        image_tokens: u.image_tokens,
        tool_use_tokens: u.tool_use_tokens,
        ..Usage::default()
    }
}

#[inline]
fn now_nanos() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos() as i64).unwrap_or(0)
}

/// Parse an ISO 8601 date-time string to epoch nanoseconds.
fn parse_iso8601(s: &str) -> Result<i64, ServerError> {
    use time::format_description::well_known::Rfc3339;
    use time::OffsetDateTime;

    let dt = OffsetDateTime::parse(s, &Rfc3339)
        .map_err(|e| ServerError::InvalidTimestamp(format!("{s}: {e}")))?;
    Ok(dt.unix_timestamp_nanos() as i64)
}

/// Build the canonical event. Body bytes are pre-serialized — only SHA256 is computed here.
fn build_llm_event(
    event: &IngestEvent,
    provider: Provider,
    model: SmolStr,
    cost: i64,
    usage: Usage,
    req_bytes: &[u8],
    resp_bytes: &[u8],
) -> Result<LlmEvent, ServerError> {
    let now_ns = now_nanos();

    let ts_ns = match &event.timestamp {
        Some(TimestampInput::EpochNanos(ns)) => *ns,
        Some(TimestampInput::Iso8601(s)) => parse_iso8601(s)?,
        None => now_ns,
    };

    let mut flags = EventFlags::empty();
    if event.flags.streaming {
        flags |= EventFlags::STREAMING;
    }
    if event.flags.tool_calls {
        flags |= EventFlags::TOOL_CALLS;
    }
    if event.flags.reasoning {
        flags |= EventFlags::REASONING;
    }
    if event.flags.stream_incomplete {
        flags |= EventFlags::STREAM_INCOMPLETE;
    }
    if event.flags.cache_used {
        flags |= EventFlags::CACHED_USED;
    }

    let error = event.error.as_ref().map(error_from_ingest);
    let method = http::Method::from_bytes(event.method.as_bytes()).unwrap_or(http::Method::POST);

    Ok(LlmEvent {
        id: EventId::new(),
        ts_ns,
        user_id: event.user_id.as_deref().map(|s| s.into()),
        api_key_id: event.api_key_id.as_deref().map(|s| s.into()),
        org_id: event.org_id.as_deref().map(|s| s.into()),
        project_id: event.project_id.as_deref().map(|s| s.into()),
        route_id: event.route_id.as_deref().unwrap_or("default").into(),
        provider,
        model,
        model_family: None,
        endpoint: SmolStr::new(&event.endpoint),
        method,
        http_status: event.http_status,
        usage,
        cost_nanodollars: cost,
        latency: Latencies {
            ttft_ms: event.latency.ttft_ms,
            total_ms: event.latency.total_ms,
            time_to_close_ms: event.latency.time_to_close_ms,
        },
        flags,
        error,
        request_sha256: sha256_bytes(req_bytes),
        response_sha256: sha256_bytes(resp_bytes),
        client_ip: event.client_ip.as_deref().and_then(|s| s.parse().ok()),
        user_agent: event.user_agent.as_deref().map(SmolStr::new),
        request_id: event.request_id.as_deref().map(SmolStr::new),
        trace_id: event.trace_id.as_deref().and_then(|s| s.parse().ok()),
        source: event.source.as_deref().map(SmolStr::new),
        ingested_at: now_ns,
    })
}

#[inline]
fn sha256_bytes(data: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hasher.finalize().into()
}

fn error_from_ingest(e: &crate::schema::IngestError) -> ProviderError {
    let msg = SmolStr::new(e.message.as_deref().unwrap_or(""));
    match e.kind.as_str() {
        "rate_limited" => ProviderError::RateLimited { retry_after: None },
        "invalid_request" => ProviderError::InvalidRequest(msg.to_string()),
        "auth_failed" => ProviderError::AuthFailed,
        "context_length_exceeded" => ProviderError::ContextLengthExceeded { limit: 0 },
        "content_filtered" => ProviderError::ContentFiltered { reason: msg },
        "upstream_timeout" => ProviderError::UpstreamTimeout,
        "upstream_unavailable" => ProviderError::UpstreamUnavailable,
        _ => ProviderError::Other { status: e.status.unwrap_or(0), message: msg },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use keplor_store::BatchConfig;

    fn test_pipeline() -> Pipeline {
        let store = Arc::new(Store::open_in_memory().unwrap());
        let writer = Arc::new(BatchWriter::new(Arc::clone(&store), BatchConfig::default()));
        let catalog = Arc::new(Catalog::load_bundled().unwrap());
        Pipeline::new(store, writer, catalog)
    }

    #[tokio::test]
    async fn ingest_minimal_event() {
        let pipeline = test_pipeline();
        let event: IngestEvent =
            serde_json::from_str(r#"{"model":"gpt-4o","provider":"openai"}"#).unwrap();
        let resp = pipeline.ingest(event).await.unwrap();
        assert!(!resp.id.is_empty());
        assert_eq!(resp.provider, "openai");
        assert_eq!(resp.model, "gpt-4o");
    }

    #[tokio::test]
    async fn ingest_with_usage_computes_cost() {
        let pipeline = test_pipeline();
        let event: IngestEvent = serde_json::from_str(
            r#"{"model":"gpt-4o","provider":"openai","usage":{"input_tokens":1000,"output_tokens":500}}"#,
        )
        .unwrap();
        let resp = pipeline.ingest(event).await.unwrap();
        assert!(resp.cost_nanodollars > 0, "cost should be > 0 for known model with usage");
    }

    #[tokio::test]
    async fn ingest_unknown_model_zero_cost() {
        let pipeline = test_pipeline();
        let event: IngestEvent = serde_json::from_str(
            r#"{"model":"totally-fake-model","provider":"openai","usage":{"input_tokens":1000}}"#,
        )
        .unwrap();
        let resp = pipeline.ingest(event).await.unwrap();
        assert_eq!(resp.cost_nanodollars, 0);
    }

    #[tokio::test]
    async fn ingest_rejects_empty_model() {
        let pipeline = test_pipeline();
        let event: IngestEvent =
            serde_json::from_str(r#"{"model":"","provider":"openai"}"#).unwrap();
        assert!(pipeline.ingest(event).await.is_err());
    }

    #[tokio::test]
    async fn ingest_stores_and_retrieves() {
        let store = Arc::new(Store::open_in_memory().unwrap());
        let writer = Arc::new(BatchWriter::new(Arc::clone(&store), BatchConfig::default()));
        let catalog = Arc::new(Catalog::load_bundled().unwrap());
        let pipeline = Pipeline::new(Arc::clone(&store), writer, catalog);

        let event: IngestEvent = serde_json::from_str(
            r#"{"model":"gpt-4o","provider":"openai","source":"litellm","user_id":"alice"}"#,
        )
        .unwrap();
        let resp = pipeline.ingest(event).await.unwrap();

        let id: EventId = resp.id.parse().unwrap();
        let loaded = store.get_event(&id).unwrap().expect("event should exist");
        assert_eq!(loaded.model, "gpt-4o");
        assert_eq!(loaded.source.as_deref(), Some("litellm"));
        assert_eq!(loaded.user_id.as_ref().map(|u| u.as_str()), Some("alice"));
        assert!(loaded.ingested_at > 0);
    }

    #[test]
    fn iso8601_parsing() {
        let ns = parse_iso8601("2024-01-15T10:30:00Z").unwrap();
        assert!(ns > 0);
        assert!(parse_iso8601("not-a-date").is_err());
    }

    #[tokio::test]
    async fn ingest_with_iso_timestamp() {
        let pipeline = test_pipeline();
        let event: IngestEvent = serde_json::from_str(
            r#"{"model":"gpt-4o","provider":"openai","timestamp":"2024-01-15T10:30:00Z"}"#,
        )
        .unwrap();
        let resp = pipeline.ingest(event).await.unwrap();
        let id: EventId = resp.id.parse().unwrap();
        let loaded = pipeline.store().get_event(&id).unwrap().unwrap();
        assert!(loaded.ts_ns > 1_705_000_000_000_000_000);
        assert!(loaded.ts_ns < 1_706_000_000_000_000_000);
    }

    #[tokio::test]
    async fn fire_and_forget_works() {
        let pipeline = test_pipeline();
        let event: IngestEvent =
            serde_json::from_str(r#"{"model":"gpt-4o","provider":"openai"}"#).unwrap();
        let resp = pipeline.ingest_fire_and_forget(event).unwrap();
        assert!(!resp.id.is_empty());
        // Give batch writer time to flush.
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}
