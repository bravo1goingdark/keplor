//! Ingestion pipeline: validate → normalise → compute cost → store.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use arc_swap::ArcSwap;
use keplor_core::{EventFlags, EventId, Latencies, LlmEvent, Provider, ProviderError, Usage};
use keplor_pricing::compute::{compute_cost, CostOpts};
use keplor_pricing::{Catalog, ModelKey};
use keplor_store::{BatchWriter, Store};
use smol_str::SmolStr;

use crate::error::ServerError;
use crate::idempotency::IdempotencyCache;
use crate::metrics::{
    self as obs, BATCH_QUEUE_CAPACITY, BATCH_QUEUE_DEPTH, INGEST_LATENCY_SECONDS, LABEL_ERROR_TYPE,
    LABEL_PROVIDER, LABEL_STAGE, LABEL_TIER,
};
use crate::normalize;
use crate::schema::{IngestEvent, IngestResponse, TimestampInput};
use crate::validate;

/// Bump the existing error counter and tag it with both `stage` and the
/// low-cardinality `error_type` derived from the [`ServerError`] variant.
///
/// Wraps the historical `keplor_events_errors_total{stage}` so existing
/// dashboards keep working — we only *append* the new label.
#[inline]
fn record_error(stage: &'static str, err: &ServerError) {
    let error_type = obs::error_type_label(err);
    metrics::counter!(
        "keplor_events_errors_total",
        LABEL_STAGE => stage,
        LABEL_ERROR_TYPE => error_type,
    )
    .increment(1);
}

/// Variant of [`record_error`] for stages that emit a fixed
/// `error_type` without a `ServerError` instance in scope (e.g. queue
/// full where the `ServerError::from` translation hasn't happened yet).
#[inline]
fn record_error_kind(stage: &'static str, error_type: &'static str) {
    metrics::counter!(
        "keplor_events_errors_total",
        LABEL_STAGE => stage,
        LABEL_ERROR_TYPE => error_type,
    )
    .increment(1);
}

/// Default cost options — reused across all events to avoid per-call construction.
static DEFAULT_COST_OPTS: CostOpts = CostOpts {
    is_batch: false,
    service_tier: keplor_pricing::compute::ServiceTier::Standard,
    inference_geo: keplor_pricing::compute::InferenceGeo::Us,
    cache_ttl: keplor_pricing::compute::CacheTtl::Minutes5,
    context_bucket: keplor_pricing::compute::ContextBucket::Standard,
};

/// Default per-event write timeout when the operator hasn't configured
/// `pipeline.write_timeout_secs` (e.g. tests or benches that build a
/// `Pipeline` directly via `Pipeline::new`).
const DEFAULT_WRITE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Shared, atomically-swappable pricing catalog. Wraps the
/// catalog in an [`ArcSwap`] so a background refresh task can hot-swap
/// it without coordinating with in-flight ingest tasks.
pub type SharedCatalog = Arc<ArcSwap<Catalog>>;

/// Shared state for the pipeline.
#[derive(Clone)]
pub struct Pipeline {
    store: Arc<Store>,
    writer: Arc<BatchWriter>,
    catalog: SharedCatalog,
    idempotency: Option<Arc<IdempotencyCache>>,
    /// Maximum DB size in bytes. 0 = unlimited.
    max_db_bytes: u64,
    /// How long a single ingest request may wait for the BatchWriter
    /// flush before returning 500. Bounds worst-case latency under
    /// back-pressure.
    write_timeout: std::time::Duration,
}

impl Pipeline {
    /// Create a new pipeline with the given store, batch writer, and pricing catalog.
    /// Wraps the supplied `Arc<Catalog>` in a fresh `ArcSwap` so the
    /// catalog can be hot-swapped via [`Pipeline::catalog_handle`].
    pub fn new(store: Arc<Store>, writer: Arc<BatchWriter>, catalog: Arc<Catalog>) -> Self {
        let shared: SharedCatalog = Arc::new(ArcSwap::new(catalog));
        Self::with_shared_catalog(store, writer, shared)
    }

    /// Like [`Pipeline::new`], but the caller already holds the
    /// [`SharedCatalog`] (because they need to hand the same handle to
    /// a refresh task that swaps the catalog while requests are in
    /// flight).
    pub fn with_shared_catalog(
        store: Arc<Store>,
        writer: Arc<BatchWriter>,
        catalog: SharedCatalog,
    ) -> Self {
        Self {
            store,
            writer,
            catalog,
            idempotency: None,
            max_db_bytes: 0,
            write_timeout: DEFAULT_WRITE_TIMEOUT,
        }
    }

    /// Borrow the swappable catalog handle so a background refresh
    /// task can `store(...)` a fresh catalog without disturbing
    /// in-flight requests.
    pub fn catalog_handle(&self) -> SharedCatalog {
        Arc::clone(&self.catalog)
    }

    /// Set maximum database size in megabytes. 0 = unlimited.
    pub fn with_max_db_size_mb(mut self, mb: u64) -> Self {
        self.max_db_bytes = mb * 1024 * 1024;
        self
    }

    /// Attach an idempotency cache to the pipeline.
    pub fn with_idempotency(mut self, cache: Arc<IdempotencyCache>) -> Self {
        self.idempotency = Some(cache);
        self
    }

    /// Override the per-event write timeout (default 10 s).
    pub fn with_write_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.write_timeout = timeout;
        self
    }

    /// Check if the database has exceeded the configured size limit.
    fn check_db_size(&self) -> Result<(), ServerError> {
        if self.max_db_bytes == 0 {
            return Ok(());
        }
        match self.store.db_size_bytes() {
            Ok(size) if size >= self.max_db_bytes => {
                let err = ServerError::StorageFull(format!(
                    "database size {:.1} MB exceeds limit of {:.1} MB",
                    size as f64 / (1024.0 * 1024.0),
                    self.max_db_bytes as f64 / (1024.0 * 1024.0),
                ));
                record_error("storage_full", &err);
                Err(err)
            },
            _ => Ok(()),
        }
    }

    /// Process a single ingestion event — durable write (awaits flush).
    ///
    /// When `authenticated_key_id` is `Some`, it overrides the
    /// client-provided `api_key_id` to prevent spoofing.
    ///
    /// When `idempotency_key` is `Some` and a cached response exists, the
    /// cached response is returned without creating a new event.
    ///
    /// Times out after 10 seconds to prevent indefinite hangs if the
    /// batch writer stalls.
    ///
    /// Not `#[instrument]`-ed — the HTTP-layer `TraceLayer` already
    /// wraps every request in an `http_request` span carrying
    /// `request_id`/`method`/`uri`. The pipeline-internal span was
    /// duplicate work paid 50K+ times/sec under load.
    pub async fn ingest(
        &self,
        event: IngestEvent,
        authenticated_key_id: Option<&str>,
        idempotency_key: Option<&str>,
        tier: &str,
    ) -> Result<IngestResponse, ServerError> {
        // Check idempotency cache.
        if let (Some(key), Some(cache)) = (idempotency_key, &self.idempotency) {
            if let Some(cached) = cache.get(key) {
                return Ok(cached);
            }
        }

        self.check_db_size()?;

        let start = std::time::Instant::now();
        let (llm_event, provider, model, cost) =
            self.process_event(event, authenticated_key_id, tier)?;

        // Snapshot queue depth on every enqueue so the gauge reflects
        // back-pressure even when no flush has occurred recently.
        self.update_queue_metrics();

        let id = tokio::time::timeout(self.write_timeout, self.writer.write(llm_event))
            .await
            .map_err(|_| {
                ServerError::Internal(format!(
                    "write timed out after {}s",
                    self.write_timeout.as_secs()
                ))
            })?
            .map_err(|e| {
                let err = ServerError::from(e);
                record_error("store", &err);
                err
            })?;

        let elapsed = start.elapsed();
        // Legacy histogram retained for dashboard continuity.
        metrics::histogram!("keplor_ingest_duration_seconds").record(elapsed.as_secs_f64());
        // Per-tier latency histogram. Tier is bounded (3-5 values) and
        // provider is &'static str — no per-event allocation. Model
        // breakdown is on the events_ingested_total counter, where
        // high-cardinality labels are cheap.
        metrics::histogram!(
            INGEST_LATENCY_SECONDS,
            LABEL_TIER => tier.to_owned(),
            LABEL_PROVIDER => provider.id_key(),
        )
        .record(elapsed.as_secs_f64());
        self.emit_metrics(&provider, &model);

        let resp = IngestResponse {
            id: id.to_string(),
            cost_nanodollars: cost,
            model: model.clone(),
            provider: SmolStr::new_static(provider.id_key()),
        };

        // Store in idempotency cache.
        if let (Some(key), Some(cache)) = (idempotency_key, &self.idempotency) {
            cache.insert(key, resp.clone());
        }

        Ok(resp)
    }

    /// Refresh the bounded-channel depth + capacity gauges. The gauges
    /// are sampled on enqueue (here) and on dequeue (inside the
    /// `BatchWriter` flush loop), giving a near-real-time view of
    /// back-pressure between flush cycles.
    #[inline]
    fn update_queue_metrics(&self) {
        metrics::gauge!(BATCH_QUEUE_DEPTH).set(self.writer.queue_depth() as f64);
        metrics::gauge!(BATCH_QUEUE_CAPACITY).set(self.writer.max_capacity() as f64);
    }

    /// Process a batch of events with durable writes — all events are sent
    /// to the channel first, then all flush confirmations are awaited
    /// concurrently. This avoids the serial-await bottleneck where each
    /// event in a durable batch waited for its own 50ms flush cycle.
    pub async fn ingest_batch_durable(
        &self,
        events: Vec<IngestEvent>,
        authenticated_key_id: Option<&str>,
        tier: &str,
    ) -> Vec<Result<IngestResponse, ServerError>> {
        if let Err(e) = self.check_db_size() {
            return events.iter().map(|_| Err(ServerError::StorageFull(e.to_string()))).collect();
        }

        let mut llm_events = Vec::with_capacity(events.len());
        let mut responses = Vec::with_capacity(events.len());
        // Track which original indices succeeded validation.
        let mut ok_indices = Vec::with_capacity(events.len());
        let event_count = events.len();
        let mut results: Vec<Option<Result<IngestResponse, ServerError>>> =
            (0..event_count).map(|_| None).collect();

        for (i, event) in events.into_iter().enumerate() {
            match self.process_event(event, authenticated_key_id, tier) {
                Ok((llm_event, provider, model, cost)) => {
                    responses.push((provider, model, cost));
                    llm_events.push(llm_event);
                    ok_indices.push(i);
                },
                Err(e) => {
                    record_error("validation", &e);
                    results[i] = Some(Err(e));
                },
            }
        }

        let batch_start = std::time::Instant::now();
        // Snapshot queue depth before the burst hits the channel.
        self.update_queue_metrics();

        // Send all valid events and await flush concurrently.
        let write_timeout = self.write_timeout;
        let write_results = tokio::time::timeout(write_timeout, self.writer.write_many(llm_events))
            .await
            .unwrap_or_else(|_| {
                let msg = format!("write timed out after {}s", write_timeout.as_secs());
                ok_indices
                    .iter()
                    .map(move |_| Err(keplor_store::StoreError::Internal(msg.clone())))
                    .collect()
            });

        // One latency sample per accepted event in the batch — keeps
        // the histogram comparable to the single-ingest path.
        let elapsed = batch_start.elapsed().as_secs_f64();

        for (j, write_result) in write_results.into_iter().enumerate() {
            let idx = ok_indices[j];
            let (ref provider, ref model, cost) = responses[j];
            match write_result {
                Ok(id) => {
                    metrics::histogram!(
                        INGEST_LATENCY_SECONDS,
                        LABEL_TIER => tier.to_owned(),
                        LABEL_PROVIDER => provider.id_key(),
                    )
                    .record(elapsed);
                    self.emit_metrics(provider, model);
                    results[idx] = Some(Ok(IngestResponse {
                        id: id.to_string(),
                        cost_nanodollars: cost,
                        model: model.clone(),
                        provider: SmolStr::new_static(provider.id_key()),
                    }));
                },
                Err(e) => {
                    let err = ServerError::from(e);
                    record_error("store", &err);
                    results[idx] = Some(Err(err));
                },
            }
        }

        results
            .into_iter()
            .map(|r| r.unwrap_or_else(|| Err(ServerError::Internal("unreachable".into()))))
            .collect()
    }

    /// Process and submit without awaiting flush — for batch endpoints.
    ///
    /// When `authenticated_key_id` is `Some`, it overrides the
    /// client-provided `api_key_id` to prevent spoofing.
    ///
    /// Events are queued for batched writes. If the queue is full, returns
    /// an error. Events may be lost if the server crashes before flushing.
    pub fn ingest_fire_and_forget(
        &self,
        event: IngestEvent,
        authenticated_key_id: Option<&str>,
        tier: &str,
    ) -> Result<IngestResponse, ServerError> {
        self.check_db_size()?;
        let (llm_event, provider, model, cost) =
            self.process_event(event, authenticated_key_id, tier)?;

        let id = llm_event.id;
        self.update_queue_metrics();
        self.writer.write_fire_and_forget(llm_event).map_err(|e| {
            // ChannelFull is the back-pressure signal: surface it as
            // its own error_type so dashboards can distinguish it from
            // generic store failures.
            record_error_kind("queue_full", "channel_full");
            ServerError::from(e)
        })?;

        self.emit_metrics(&provider, &model);

        Ok(IngestResponse {
            id: id.to_string(),
            cost_nanodollars: cost,
            model: model.clone(),
            provider: SmolStr::new_static(provider.id_key()),
        })
    }

    /// Core processing: validate → normalize → cost → build event.
    ///
    /// Returns `(LlmEvent, provider, model, cost)`.
    ///
    /// Not `#[instrument]`-ed — at 50 K+ ingest/s the per-call span
    /// allocation dominates the synchronous work this function does.
    /// Provider + model are recorded on the parent ingest span by
    /// the caller once they're known.
    #[inline]
    fn process_event(
        &self,
        mut event: IngestEvent,
        authenticated_key_id: Option<&str>,
        tier: &str,
    ) -> Result<(LlmEvent, Provider, SmolStr, i64), ServerError> {
        // Server-side key attribution: override client-provided api_key_id
        // with the authenticated key identity to prevent spoofing.
        if let Some(key_id) = authenticated_key_id {
            event.api_key_id = Some(key_id.to_owned());
        }
        validate::validate(&event).inspect_err(|e| {
            record_error("validation", e);
        })?;

        let provider = normalize::normalize_provider(&event.provider);
        let model = normalize::normalize_model(&event.model);

        let usage = usage_from_ingest(&event.usage);
        let cost =
            event.cost_nanodollars.unwrap_or_else(|| self.compute_cost(&provider, &model, &usage));

        let llm_event = build_llm_event(event, provider.clone(), model.clone(), cost, usage, tier)?;

        Ok((llm_event, provider, model, cost))
    }

    /// Direct store access for queries.
    pub fn store(&self) -> &Store {
        &self.store
    }

    /// Shared store handle for `spawn_blocking` closures.
    pub fn store_arc(&self) -> Arc<Store> {
        Arc::clone(&self.store)
    }

    /// Shared batch writer handle (for shutdown draining).
    pub fn writer_arc(&self) -> Arc<BatchWriter> {
        Arc::clone(&self.writer)
    }

    /// Number of events currently queued in the batch writer channel.
    pub fn queue_depth(&self) -> usize {
        self.writer.queue_depth()
    }

    /// Maximum batch writer channel capacity.
    pub fn queue_capacity(&self) -> usize {
        self.writer.max_capacity()
    }

    #[inline]
    fn compute_cost(&self, provider: &Provider, model: &str, usage: &Usage) -> i64 {
        let key = ModelKey::from_normalized(SmolStr::new(model));
        // Snapshot the catalog once per call; ArcSwap loads are
        // RCU-style and effectively free on the hot path.
        let catalog = self.catalog.load();
        match catalog.lookup(&key) {
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

/// Build the canonical event from an ingested payload.
///
/// SHA-256 of bodies is deferred to the batch writer — computing hashes
/// here would block the request thread for work the store repeats anyway
/// (it hashes individual components, not whole bodies).
fn build_llm_event(
    event: IngestEvent,
    provider: Provider,
    model: SmolStr,
    cost: i64,
    usage: Usage,
    tier: &str,
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
        // Deferred to batch writer — [0;32] signals "compute on write".
        request_sha256: [0u8; 32],
        response_sha256: [0u8; 32],
        client_ip: event.client_ip.as_deref().and_then(|s| s.parse().ok()),
        user_agent: event.user_agent.as_deref().map(SmolStr::new),
        request_id: event.request_id.as_deref().map(SmolStr::new),
        trace_id: event.trace_id.as_deref().and_then(|s| s.parse().ok()),
        source: event.source.as_deref().map(SmolStr::new),
        ingested_at: now_ns,
        // Take ownership — avoids cloning the serde_json::Value.
        metadata: event.metadata,
        tier: SmolStr::new(tier),
    })
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
        let resp = pipeline.ingest(event, None, None, "free").await.unwrap();
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
        let resp = pipeline.ingest(event, None, None, "free").await.unwrap();
        assert!(resp.cost_nanodollars > 0, "cost should be > 0 for known model with usage");
    }

    #[tokio::test]
    async fn ingest_unknown_model_zero_cost() {
        let pipeline = test_pipeline();
        let event: IngestEvent = serde_json::from_str(
            r#"{"model":"totally-fake-model","provider":"openai","usage":{"input_tokens":1000}}"#,
        )
        .unwrap();
        let resp = pipeline.ingest(event, None, None, "free").await.unwrap();
        assert_eq!(resp.cost_nanodollars, 0);
    }

    #[tokio::test]
    async fn ingest_rejects_empty_model() {
        let pipeline = test_pipeline();
        let event: IngestEvent =
            serde_json::from_str(r#"{"model":"","provider":"openai"}"#).unwrap();
        assert!(pipeline.ingest(event, None, None, "free").await.is_err());
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
        let resp = pipeline.ingest(event, None, None, "free").await.unwrap();

        // KeplorDB reads only see rotated segments; flush before query.
        store.wal_checkpoint().unwrap();

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
        let resp = pipeline.ingest(event, None, None, "free").await.unwrap();
        pipeline.store().wal_checkpoint().unwrap();
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
        let resp = pipeline.ingest_fire_and_forget(event, None, "free").unwrap();
        assert!(!resp.id.is_empty());
        // Give batch writer time to flush.
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }

    #[tokio::test]
    async fn authenticated_key_overrides_client_api_key_id() {
        let store = Arc::new(Store::open_in_memory().unwrap());
        let writer = Arc::new(BatchWriter::new(Arc::clone(&store), BatchConfig::default()));
        let catalog = Arc::new(Catalog::load_bundled().unwrap());
        let pipeline = Pipeline::new(Arc::clone(&store), writer, catalog);

        let event: IngestEvent = serde_json::from_str(
            r#"{"model":"gpt-4o","provider":"openai","api_key_id":"spoofed-key","user_id":"alice"}"#,
        )
        .unwrap();

        // Simulate server-side key attribution overriding client-provided value.
        let resp = pipeline.ingest(event, Some("real-key"), None, "pro").await.unwrap();
        store.wal_checkpoint().unwrap();
        let id: EventId = resp.id.parse().unwrap();
        let loaded = store.get_event(&id).unwrap().expect("event should exist");
        assert_eq!(
            loaded.api_key_id.as_ref().map(|k| k.as_str()),
            Some("real-key"),
            "server-injected key should override client-provided api_key_id"
        );
    }
}
