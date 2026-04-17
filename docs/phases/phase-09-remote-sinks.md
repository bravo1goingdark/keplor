# Phase 9 — Remote sinks and OTel export

**Status:** not started
**Depends on:** phases 3, 5, 6, 7
**Unlocks:** phase 11

## Goal

Keplor stays the primary store, but add fan-out to ClickHouse, S3 payload offload, Postgres, and OTLP. All optional, all lossy-fail-safe (a broken ClickHouse never blocks ingest).

## Prompt

### 1. Sink trait in `keplor-store`

```rust
#[async_trait]
pub trait RemoteSink: Send + Sync {
    async fn write_batch(
        &self,
        events: &[LlmEvent],
        blobs: &[(Sha256, &[u8])],
    ) -> Result<()>;
    fn name(&self) -> &str;
    fn health(&self) -> Health;
}
```

A `SinkManager` runs each registered sink in its own task with:
- Bounded mpsc channel (default 10k events).
- Batching: flush on 500 events or 2 s whichever first.
- Exponential-backoff retries (max 6).
- Dead-letter table `failed_sink_writes` after retry exhaustion.
- Health exposed at `/health/sinks`.

### 2. ClickHouse sink (features = ["sink-clickhouse"])

- `clickhouse` crate, native protocol, LZ4 on wire.
- Target table: `keplor.events` MergeTree `ORDER BY (ts, user_id)` with `CODEC(Delta, ZSTD)` on ts and `LowCardinality(String)` on model/provider.
- Optional `keplor.payload_blobs` table for body export (most users keep bodies local-only; make this opt-in per-route).
- Provide a `keplor clickhouse init` subcommand that prints/applies DDL.

### 3. S3 payload offload (features = ["sink-s3"])

- `aws-sdk-s3`, multipart upload for blobs > 8 MiB.
- Background sweeper: blobs older than `s3.hot_ttl_hours` (default 72h) are uploaded, then the local BLOB is replaced with a stub `{storage: External(url)}` and the SQLite BLOB column nulled.
- Reader side-effect: reading an external-storage `PayloadRef` streams from S3 through zstd-decompressor with the dict pulled from `zstd_dicts` (still local).

### 4. Postgres sink (features = ["sink-postgres"])

- `sqlx` with rustls.
- Normalized schema similar to SQLite. No body storage by default; metadata only. Useful for teams that already have a Postgres for usage reporting.

### 5. OTLP sink (features = ["sink-otlp"])

- `opentelemetry-otlp` with HTTP/protobuf exporter.
- For every `LlmEvent`, emit a single span with **both** OTel GenAI and OpenInference attributes (see `docs/architecture.md` for the full attribute list).
- Content capture is controlled by `observability.capture_messages_in_otlp = false` by default (honors privacy); turn on via env var `OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT=true` for compatibility with upstream OTel standard.
- Validated compatibility targets: Langfuse, Phoenix (Arize), LangSmith, Datadog, Honeycomb, Grafana Tempo. Add integration tests with a local Phoenix docker-compose.

### 6. Config

```toml
[[sinks]]
type = "clickhouse"
url = "tcp://clickhouse:9000"
database = "keplor"
include_payloads = false

[[sinks]]
type = "s3"
bucket = "my-keplor-archive"
hot_ttl_hours = 72

[[sinks]]
type = "otlp"
endpoint = "http://otel-collector:4318/v1/traces"
capture_messages = false
```

### 7. Tests

For each sink, a docker-compose-backed integration test in `tests/e2e/sinks/`:

- `clickhouse`
- `minio-as-s3`
- `postgres`
- `jaeger-as-otlp`

Run opt-in via `cargo test --features full-e2e`.

## Acceptance criteria

- [ ] Test matrix green for all four sinks
- [ ] Broken-sink isolation verified: kill a sink mid-run, proxy keeps serving, dead-letter table fills, no client impact
- [ ] OTLP output ingests cleanly into a local Phoenix instance (manual verification; screenshots in `docs/integrations/phoenix.md`)
- [ ] `cargo test --features full-e2e` green in CI on a self-hosted runner (optional gate)
