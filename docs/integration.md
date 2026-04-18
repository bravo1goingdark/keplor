# Keplor Integration Guide

Keplor is a lightweight observability and cost-accounting server for LLM traffic. Any service that calls an LLM provider can POST event data to Keplor and get back accurate cost tracking, usage rollups, and full prompt/completion storage.

This document covers everything a service needs to integrate.

---

## Quick Start

Ingest a single event (no auth, default config):

```bash
curl -X POST http://localhost:8080/v1/events \
  -H "Content-Type: application/json" \
  -d '{
    "model": "gpt-4o",
    "provider": "openai",
    "usage": { "input_tokens": 1000, "output_tokens": 500 }
  }'
```

Response (`201 Created`):

```json
{
  "id": "01JA2B3C4D5E6F7G8H9J0KMNPQ",
  "cost_nanodollars": 6250000,
  "model": "gpt-4o",
  "provider": "openai"
}
```

Keplor computed cost automatically from its bundled pricing catalog. The `id` is a ULID (time-sortable unique identifier).

---

## Authentication

### Header Format

```
Authorization: Bearer <secret>
```

### Key Configuration

Keys are configured in `keplor.toml` or via environment variables. Two formats are supported:

**Simple format** (backward compatible):

```toml
[auth]
api_keys = [
  "prod-svc:sk-prod-abc123",       # explicit id:secret
  "staging-svc:sk-staging-xyz789",  # explicit id:secret
  "sk-legacy-key-no-id"             # bare secret (auto-derives ID)
]
```

**Extended format** (with retention tier assignment):

```toml
[[auth.api_key_entries]]
id = "prod-svc"
secret = "sk-prod-abc123"
tier = "pro"                        # maps to a retention tier

[[auth.api_key_entries]]
id = "free-user"
secret = "sk-free-xyz789"
tier = "free"
```

Both formats can be combined. Simple-format keys default to the `default_tier` from `[retention]` config.

| Format | Example | Key ID | Tier |
|--------|---------|--------|------|
| `id:secret` | `"prod-svc:sk-abc123"` | `prod-svc` | `default_tier` |
| bare secret | `"sk-abc123"` | `key_<first8hex_sha256>` (auto-derived) | `default_tier` |
| extended | `{ id, secret, tier }` | `id` value | `tier` value |

### Server-Side Key Attribution

When auth is enabled, Keplor **overrides** the client-provided `api_key_id` field with the authenticated key's ID. This prevents clients from spoofing attribution. Cost rollups, quotas, and billing queries are always tied to the actual key that authenticated.

### Open Mode

When `api_keys` is empty (the default), authentication is disabled. All requests are accepted without a Bearer token, and `api_key_id` is taken as-is from the client.

### Auth Failures

| Scenario | HTTP Status | Prometheus Metric |
|----------|-------------|-------------------|
| Missing/empty Bearer token | `401 Unauthorized` | `keplor_auth_failures_total{reason="missing"}` |
| Invalid token | `401 Unauthorized` | `keplor_auth_failures_total{reason="invalid"}` |
| Successful auth | (request proceeds) | `keplor_auth_successes_total` |

---

## Endpoints

### POST /v1/events

Ingest a single LLM event. Waits for durable storage (up to 10s timeout).

**Auth:** Required when keys are configured.

**Request Body:** JSON `IngestEvent` (see [Schema Reference](#schema-reference)).

**Response:** `201 Created`

```json
{
  "id": "01JA2B3C4D5E6F7G8H9J0KMNPQ",
  "cost_nanodollars": 6250000,
  "model": "gpt-4o",
  "provider": "openai"
}
```

### POST /v1/events/batch

Ingest up to 10,000 events in one request. By default, fire-and-forget: events are validated and queued but not guaranteed durable until the next batch flush (~50ms). Set the `X-Keplor-Durable: true` header to await flush confirmation for each event before responding (slower, but every accepted event is guaranteed durable).

**Auth:** Required when keys are configured.

**Request Body:**

```json
{
  "events": [
    { "model": "gpt-4o", "provider": "openai", "usage": { "input_tokens": 500 } },
    { "model": "claude-sonnet-4-20250514", "provider": "anthropic", "usage": { "input_tokens": 800, "output_tokens": 200 } }
  ]
}
```

**Response:** `201 Created` (all accepted) or `207 Multi-Status` (partial failure)

```json
{
  "results": [
    { "id": "01JA...", "cost_nanodollars": 1250000, "model": "gpt-4o", "provider": "openai" },
    { "error": "model is required" }
  ],
  "accepted": 1,
  "rejected": 1
}
```

### GET /v1/events

Query stored events with filters and cursor-based pagination.

**Auth:** Required when keys are configured.

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `user_id` | string | | Filter by user ID |
| `api_key_id` | string | | Filter by API key ID |
| `model` | string | | Filter by model name |
| `provider` | string | | Filter by provider |
| `source` | string | | Filter by ingestion source |
| `from` | int64 | | Events on or after this epoch nanosecond timestamp |
| `to` | int64 | | Events on or before this epoch nanosecond timestamp |
| `status_min` | uint16 | | Only events with `http_status >= status_min` |
| `status_max` | uint16 | | Only events with `http_status < status_max` |
| `user_tag` | string | | Filter by `metadata.user_tag` |
| `session_tag` | string | | Filter by `metadata.session_tag` |
| `limit` | uint32 | 50 | Results per page (max 1000) |
| `cursor` | int64 | | Pagination cursor (`ts_ns` from previous page) |

**Response:** `200 OK`

```json
{
  "events": [
    {
      "id": "01JA...",
      "timestamp": 1700000000000000000,
      "model": "gpt-4o",
      "provider": "openai",
      "user_id": "alice",
      "api_key_id": "prod-svc",
      "endpoint": "/v1/chat/completions",
      "http_status": 200,
      "usage": {
        "input_tokens": 1000,
        "output_tokens": 500,
        "cache_read_input_tokens": 200,
        "reasoning_tokens": 0
      },
      "cost_nanodollars": 6250000,
      "latency_ttft_ms": 25,
      "latency_total_ms": 300,
      "streaming": true,
      "source": "litellm",
      "error": null,
      "metadata": { "custom_field": "value" }
    }
  ],
  "cursor": 1700000000000000000,
  "has_more": true
}
```

Paginate by passing the returned `cursor` value as the `cursor` parameter in the next request.

### GET /v1/quota

Real-time cost and event count for a user or key within a time window.

**Auth:** Required when keys are configured.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `user_id` | string | One of `user_id` or `api_key_id` required | Filter by user |
| `api_key_id` | string | | Filter by API key |
| `from` | int64 | Yes | Start epoch nanoseconds |

**Response:** `200 OK`

```json
{
  "cost_nanodollars": 5000000,
  "event_count": 250
}
```

### GET /v1/rollups

Pre-aggregated daily rollups. Refreshed every 60 seconds.

**Auth:** Required when keys are configured.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `user_id` | string | | | Filter by user |
| `api_key_id` | string | | | Filter by API key |
| `from` | int64 | Yes | | Start epoch nanoseconds |
| `to` | int64 | Yes | | End epoch nanoseconds |
| `limit` | uint32 | | 100 | Max rows (max 1000) |
| `offset` | uint32 | | 0 | Offset for pagination |

**Response:** `200 OK`

```json
{
  "rollups": [
    {
      "day": 1700000000,
      "user_id": "alice",
      "api_key_id": "prod-svc",
      "provider": "openai",
      "model": "gpt-4o",
      "event_count": 50,
      "error_count": 2,
      "input_tokens": 10000,
      "output_tokens": 5000,
      "cache_read_input_tokens": 2000,
      "cache_creation_input_tokens": 0,
      "cost_nanodollars": 50000000
    }
  ],
  "has_more": false
}
```

### GET /v1/stats

Aggregated statistics over a time range, optionally grouped by model.

**Auth:** Required when keys are configured.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `user_id` | string | | | Filter by user |
| `api_key_id` | string | | | Filter by API key |
| `from` | int64 | Yes | | Start epoch nanoseconds |
| `to` | int64 | Yes | | End epoch nanoseconds |
| `provider` | string | | | Filter by provider |
| `group_by` | string | | | Set to `"model"` for per-model breakdown |
| `limit` | uint32 | | 100 | Max rows (max 1000) |
| `offset` | uint32 | | 0 | Offset for pagination |

**Response:** `200 OK`

```json
{
  "stats": [
    {
      "provider": "openai",
      "model": "gpt-4o",
      "event_count": 100,
      "error_count": 3,
      "input_tokens": 50000,
      "output_tokens": 25000,
      "cache_read_input_tokens": 5000,
      "cache_creation_input_tokens": 1000,
      "cost_nanodollars": 250000000
    }
  ],
  "has_more": false
}
```

### DELETE /v1/events/:id

Delete a single event by ID. Cleans up blob references and orphaned blobs.

**Auth:** Required when keys are configured.

**Response:** `204 No Content` if deleted, `404 Not Found` if not found.

### DELETE /v1/events?older_than_days=N

Bulk delete events older than N days. Equivalent to `keplor gc` via HTTP.

**Auth:** Required when keys are configured.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `older_than_days` | uint32 | Yes | Delete events older than this many days (must be > 0) |

**Response:** `200 OK`

```json
{
  "events_deleted": 1234,
  "blobs_deleted": 56
}
```

### GET /v1/events/export

Stream all matching events as JSON Lines (`application/x-ndjson`). Same filters as `GET /v1/events` but with no result-set limit. Each line is one JSON event object.

**Auth:** Required when keys are configured.

**Parameters:** Same as `GET /v1/events` (except `limit` and `cursor` are ignored).

**Response:** `200 OK` with `Content-Type: application/x-ndjson`

### GET /health

Liveness probe with DB and queue status. No auth.

```json
{
  "status": "ok",
  "version": "0.1.0",
  "db": "connected",
  "queue_depth": 42,
  "queue_capacity": 8192,
  "queue_utilization_pct": 0
}
```

Returns `503` with `"status": "degraded"` if the database is unreachable. The `queue_depth` / `queue_capacity` fields show how full the batch writer channel is — useful for back-pressure monitoring.

### GET /metrics

Prometheus metrics export. No auth. Returns `text/plain` in Prometheus exposition format.

---

## Schema Reference

### IngestEvent

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `model` | string | Yes | | Model name (e.g., `"gpt-4o"`, `"claude-sonnet-4-20250514"`) |
| `provider` | string | Yes | | Provider key (see [Supported Providers](#supported-providers)) |
| `cost_nanodollars` | int64 | No | computed | Override Keplor's cost calculation with your own value |
| `usage` | object | No | all zeros | Token usage counters (see below) |
| `latency` | object | No | all zeros | Latency breakdown (see below) |
| `timestamp` | int64 or string | No | server time | Epoch nanoseconds or ISO 8601 string |
| `method` | string | No | `"POST"` | HTTP method of the upstream call |
| `endpoint` | string | No | `""` | API endpoint path (e.g., `"/v1/chat/completions"`) |
| `http_status` | uint16 | No | | HTTP status code from the upstream |
| `source` | string | No | | Name of the system sending this event |
| `user_id` | string | No | | User identity for cost attribution |
| `api_key_id` | string | No | | API key identity (overridden by server when auth is enabled) |
| `org_id` | string | No | | Organization ID |
| `project_id` | string | No | | Project ID |
| `route_id` | string | No | `"default"` | Logical route name (e.g., `"chat"`, `"embeddings"`) |
| `flags` | object | No | all false | Boolean signals (see below) |
| `error` | object | No | | Upstream error details (see below) |
| `trace_id` | string | No | | W3C trace-context trace ID |
| `request_id` | string | No | | Provider-returned request ID |
| `client_ip` | string | No | | Client source IP |
| `user_agent` | string | No | | Client user-agent string |
| `request_body` | any JSON | No | | Raw request body (stored compressed) |
| `response_body` | any JSON | No | | Raw response body (stored compressed) |
| `metadata` | any JSON | No | | Arbitrary metadata (stored, queryable via `user_tag`/`session_tag`) |

### Usage Object

All fields are `uint32`, default `0`.

| Field | Description |
|-------|-------------|
| `input_tokens` | Input/prompt token count |
| `output_tokens` | Output/completion token count |
| `cache_read_input_tokens` | Tokens served from provider cache |
| `cache_creation_input_tokens` | Tokens written to provider cache |
| `reasoning_tokens` | Tokens used for chain-of-thought reasoning |
| `audio_input_tokens` | Audio input tokens |
| `audio_output_tokens` | Audio output tokens |
| `image_tokens` | Image/vision tokens |
| `tool_use_tokens` | Tokens for tool/function calls |

### Latency Object

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `ttft_ms` | uint32 | null | Time to first byte (milliseconds) |
| `total_ms` | uint32 | 0 | End-to-end latency (milliseconds) |
| `time_to_close_ms` | uint32 | null | Time from last token to stream close |

### Flags Object

All fields are `bool`, default `false`.

| Field | Description |
|-------|-------------|
| `streaming` | Response was streamed (SSE/event-stream) |
| `tool_calls` | Request included tool/function calls |
| `reasoning` | Model used extended thinking/reasoning |
| `stream_incomplete` | Stream ended prematurely |
| `cache_used` | Response (partially) served from cache |

### Error Object

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `kind` | string | Yes | Error category (e.g., `"rate_limited"`, `"auth_failed"`, `"context_length"`) |
| `message` | string | No | Human-readable error message |
| `status` | uint16 | No | HTTP status of the error response |

### Timestamp Format

The `timestamp` field accepts two formats:

```json
{ "timestamp": 1700000000000000000 }
```

```json
{ "timestamp": "2024-01-15T10:30:00Z" }
```

When omitted, Keplor uses the server's wall-clock time at ingestion.

### Validation Rules

| Rule | Error |
|------|-------|
| `model` cannot be empty | `400: model is required` |
| `model` max 256 characters | `400: model exceeds 256 characters` |
| `provider` cannot be empty | `400: provider is required` |
| `provider` max 128 characters | `400: provider exceeds 128 characters` |
| Batch max 10,000 events | `400: batch size N exceeds maximum 10000` |
| Token fields max 10,000,000 each | `400: input_tokens = N exceeds maximum 10000000` |
| `cost_nanodollars` must be non-negative | `400: cost_nanodollars must not be negative` |
| `cost_nanodollars` max 1,000,000,000,000 ($1,000) | `400: cost_nanodollars N exceeds maximum` |
| Epoch nanos timestamp must be after 2020-01-01 | `400: timestamp is before 2020-01-01` |
| Epoch nanos timestamp must be within 24h of now | `400: timestamp is more than 24 hours in the future` |
| `user_id`, `api_key_id`, `org_id`, `project_id`, `route_id` max 256 chars | `400: user_id exceeds 256 characters` |
| `endpoint` max 512 characters | `400: endpoint exceeds 512 characters` |
| `metadata` JSON max 64 KB | `400: metadata JSON exceeds 65536 bytes` |

---

## Supported Providers

| Provider Key | Service | Notes |
|-------------|---------|-------|
| `openai` | OpenAI (api.openai.com) | Chat Completions + Responses API |
| `anthropic` | Anthropic (api.anthropic.com) | Messages API |
| `gemini` | Google AI Studio | generateContent / streamGenerateContent |
| `vertex_ai` | Google Vertex AI | Same as gemini, different endpoint |
| `bedrock` | AWS Bedrock | Converse/ConverseStream + InvokeModel |
| `azure` | Azure OpenAI | OpenAI-compatible, Azure-hosted |
| `mistral` | Mistral AI | api.mistral.ai |
| `groq` | Groq | api.groq.com |
| `xai` | xAI Grok | api.x.ai |
| `deepseek` | DeepSeek | api.deepseek.com |
| `cohere` | Cohere v2 | api.cohere.com |
| `ollama` | Ollama (local) | localhost:11434 |

Any unrecognized provider string is treated as **OpenAI-compatible** and processed accordingly. Provider matching is case-insensitive.

---

## Cost Accounting

### Nanodollars

All costs are stored as **int64 nanodollars** (1 nanodollar = 10^-9 USD). This avoids floating-point precision issues.

| Value | USD Equivalent |
|-------|---------------|
| `1,000,000,000` | $1.00 |
| `1,000,000` | $0.001 |
| `1,000` | $0.000001 |

### Automatic Cost Computation

When `cost_nanodollars` is not provided in the event, Keplor computes it from:

1. The **model name** (matched against the bundled LiteLLM pricing catalog)
2. The **usage** token counts
3. Provider-specific adjustments:
   - Prompt caching discounts (Anthropic, OpenAI)
   - Reasoning token pricing (o1, o3, etc.)
   - Batch API discounts
   - Audio/image token pricing

### Cost Override

To bypass Keplor's calculation (e.g., if your application already computed cost):

```json
{
  "model": "gpt-4o",
  "provider": "openai",
  "cost_nanodollars": 5000000,
  "usage": { "input_tokens": 1000 }
}
```

When `cost_nanodollars` is present, Keplor stores that value directly.

### Unknown Models

If the model is not in the pricing catalog, cost is `0`. The event is still stored with all other fields.

---

## Configuration

### Config File (keplor.toml)

```toml
[server]
listen_addr = "0.0.0.0:8080"      # default
shutdown_timeout_secs = 25          # drain batch writer + WAL checkpoint on stop
request_timeout_secs = 30           # per-request timeout (slow clients dropped)
max_connections = 10000             # concurrent connection limit

[storage]
db_path = "keplor.db"              # SQLite file path
retention_days = 90                 # legacy global GC (0 = disabled; prefer [retention] tiers)
wal_checkpoint_secs = 300           # WAL truncation interval (0 = disabled)
gc_interval_secs = 3600            # how often GC runs (0 = disabled)

[auth]
api_keys = []                       # simple format (open mode when empty)

# Extended format — assign retention tiers per key:
# [[auth.api_key_entries]]
# id = "prod-svc"
# secret = "sk-prod-abc123"
# tier = "pro"

[retention]
default_tier = "free"              # tier for simple-format keys & unauthenticated requests

[[retention.tiers]]
name = "free"
days = 7                           # 0 = keep forever

[[retention.tiers]]
name = "pro"
days = 90

# Add custom tiers:
# [[retention.tiers]]
# name = "team"
# days = 180

[pipeline]
batch_size = 64                     # events per batched write (max 100,000)
max_body_bytes = 10485760           # 10 MB max request body (max 100 MB)

[idempotency]
enabled = true                      # default
ttl_secs = 300                      # cache TTL (5 minutes)
max_entries = 100000                # LRU cache capacity

[rate_limit]
enabled = false                     # default (disabled)
requests_per_second = 100.0         # per API key
burst = 200                         # max burst size

# Optional: offload blobs to S3/R2 (requires --features s3)
# [blob_storage]
# bucket = "keplor-blobs"
# endpoint = "https://<account>.r2.cloudflarestorage.com"
# region = "auto"
# access_key_id = "..."
# secret_access_key = "..."

# Optional TLS — when present, server listens with HTTPS
# [tls]
# cert_path = "/etc/keplor/cert.pem"
# key_path = "/etc/keplor/key.pem"
```

### Blob Storage (S3 / Cloudflare R2 / MinIO)

By default, Keplor stores request/response bodies in the SQLite database alongside event metadata. For deployments where disk is constrained (e.g. free-tier VMs) or you want to decouple storage, you can offload blob data to any S3-compatible object store.

**What moves:** Only the compressed request/response body bytes. Event metadata (timestamps, tokens, cost, user IDs) stays in SQLite for fast queries.

**What stays:** All query, stats, rollup, and quota endpoints work identically. The only difference is that viewing full request/response bodies (`get_component`) fetches from the object store instead of SQLite.

#### Build with S3 support

```bash
cargo build --release --target x86_64-unknown-linux-musl -p keplor-cli \
  --features mimalloc,s3
```

#### Cloudflare R2 setup

R2 has a generous free tier (10 GB storage, no egress fees) and is S3-compatible.

1. Create a bucket in the Cloudflare dashboard (e.g. `keplor-blobs`)
2. Create an R2 API token with read/write permissions
3. Add to `keplor.toml`:

```toml
[blob_storage]
bucket = "keplor-blobs"
endpoint = "https://<account-id>.r2.cloudflarestorage.com"
region = "auto"
access_key_id = "your-r2-access-key"
secret_access_key = "your-r2-secret-key"
```

#### AWS S3 setup

```toml
[blob_storage]
bucket = "keplor-blobs"
endpoint = "https://s3.us-east-1.amazonaws.com"
region = "us-east-1"
access_key_id = "AKIA..."
secret_access_key = "..."
```

#### MinIO (self-hosted S3)

```toml
[blob_storage]
bucket = "keplor-blobs"
endpoint = "http://localhost:9000"
region = "us-east-1"
access_key_id = "minioadmin"
secret_access_key = "minioadmin"
path_style = true                   # required for MinIO
```

#### Configuration reference

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `bucket` | string | | Bucket name |
| `endpoint` | string | | S3 endpoint URL |
| `region` | string | | Region (`"auto"` for R2, `"us-east-1"` for AWS) |
| `access_key_id` | string | | Access key |
| `secret_access_key` | string | | Secret key |
| `prefix` | string | `""` | Optional key prefix (e.g. `"blobs/"`) |
| `path_style` | bool | `false` | Use path-style addressing (required for MinIO) |

#### How deduplication works

Blobs are keyed by their SHA-256 hash. Identical payloads (e.g. repeated system prompts) produce the same key, so S3 PUTs are naturally idempotent. No coordination needed.

#### Garbage collection with external blobs

When events are deleted (via retention GC or the DELETE API), Keplor:
1. Decrements the blob's reference count in SQLite
2. If the count reaches zero, deletes the blob from the object store

Failed external deletes are logged as warnings but don't block GC. Orphaned blobs in S3 waste storage but don't cause correctness issues.

#### Migration from embedded to S3

Existing blobs in SQLite are **not** automatically migrated to S3. New events will write to S3; old events continue reading from SQLite. To fully migrate, export and re-ingest, or run a migration script against the `payload_blobs` table.

### Environment Variable Overrides

Any config field can be overridden with `KEPLOR_<SECTION>_<FIELD>`:

```bash
KEPLOR_SERVER_LISTEN_ADDR=0.0.0.0:9000
KEPLOR_STORAGE_DB_PATH=/data/keplor.db
KEPLOR_STORAGE_RETENTION_DAYS=30
KEPLOR_PIPELINE_BATCH_SIZE=128
KEPLOR_AUTH_API_KEYS=prod-svc:sk-abc,staging-svc:sk-xyz
```

### JSON Structured Logging

For log aggregation systems (Loki, Datadog, CloudWatch), start with:

```bash
keplor run --json-logs
```

### Graceful Shutdown

On SIGINT/SIGTERM, Keplor:
1. Stops accepting new connections
2. Drains the batch writer (flushes all pending events to SQLite)
3. Runs a final WAL checkpoint
4. Exits cleanly

The drain step waits up to `shutdown_timeout_secs`. If it times out, a warning is logged and some events may be lost.

---

## Error Handling

### Error Response Format

All errors return JSON:

```json
{ "error": "<message>" }
```

### Status Codes

| Code | Meaning | When |
|------|---------|------|
| `200` | OK | Query, stats, export, or bulk delete succeeded |
| `201` | Created | Event(s) successfully ingested |
| `204` | No Content | Single event deleted successfully |
| `207` | Multi-Status | Batch with partial failures |
| `400` | Bad Request | Validation failure, invalid JSON, batch too large |
| `401` | Unauthorized | Missing or invalid API key |
| `404` | Not Found | Event ID does not exist (DELETE) |
| `408` | Request Timeout | Request exceeded `request_timeout_secs` |
| `422` | Unprocessable Entity | Deserialization error (missing required fields) |
| `429` | Too Many Requests | Per-key rate limit exceeded (includes `Retry-After` header) |
| `503` | Service Unavailable | Batch writer channel full (back-pressure) or channel closed |
| `507` | Insufficient Storage | Database size limit exceeded (`storage.max_db_size_mb`) |
| `500` | Internal Server Error | Database or server failure (details logged, not exposed) |

### Retry Guidance

| Status | Retry? | Notes |
|--------|--------|-------|
| `200`, `201`, `204`, `207` | No | Success |
| `400` | No | Fix the request |
| `401` | No | Fix your API key |
| `404` | No | Event does not exist |
| `408` | Yes | Request was too slow; retry immediately |
| `422` | No | Fix the JSON payload |
| `429` | Yes | Wait for `Retry-After` seconds, then retry |
| `503` | Yes | Server is overloaded; retry with exponential backoff |
| `507` | Yes | Run GC or increase `max_db_size_mb`, then retry |
| `500` | Yes | Retry with exponential backoff (1s, 2s, 4s, ...) |

### Request Headers

| Header | Direction | Description |
|--------|-----------|-------------|
| `Authorization` | Request | `Bearer <secret>` (required when keys configured) |
| `Idempotency-Key` | Request | Optional. Prevents duplicate event creation on retries. Cached for 5 minutes (configurable). |
| `X-Keplor-Durable` | Request | Set to `true` on batch endpoint to await flush confirmation for each event. Default: fire-and-forget. |
| `X-Request-Id` | Both | If sent in the request, echoed back. If absent, Keplor generates a ULID and returns it in the response. Useful for correlating logs. |
| `Retry-After` | Response | Returned with `429` responses. Seconds until the rate limit resets. |

---

## Examples

### Python (requests)

```python
import requests
import time

KEPLOR_URL = "http://localhost:8080"
API_KEY = "sk-your-key"  # omit header if open mode

headers = {
    "Authorization": f"Bearer {API_KEY}",
    "Content-Type": "application/json",
}

# Ingest a single event
resp = requests.post(f"{KEPLOR_URL}/v1/events", headers=headers, json={
    "model": "gpt-4o",
    "provider": "openai",
    "usage": {"input_tokens": 1500, "output_tokens": 800},
    "latency": {"ttft_ms": 30, "total_ms": 450},
    "http_status": 200,
    "user_id": "user_42",
    "source": "my-app",
    "endpoint": "/v1/chat/completions",
    "flags": {"streaming": True},
})
print(resp.json())
# {"id": "01JA...", "cost_nanodollars": 9250000, "model": "gpt-4o", "provider": "openai"}

# Ingest a batch
resp = requests.post(f"{KEPLOR_URL}/v1/events/batch", headers=headers, json={
    "events": [
        {"model": "gpt-4o", "provider": "openai", "usage": {"input_tokens": 500}},
        {"model": "claude-sonnet-4-20250514", "provider": "anthropic", "usage": {"input_tokens": 1000, "output_tokens": 300}},
    ]
})
print(resp.json())
# {"results": [...], "accepted": 2, "rejected": 0}

# Query cost for a user this month
now_ns = int(time.time() * 1e9)
month_ago_ns = now_ns - (30 * 86400 * int(1e9))
resp = requests.get(f"{KEPLOR_URL}/v1/quota", headers=headers, params={
    "user_id": "user_42",
    "from": month_ago_ns,
})
print(resp.json())
# {"cost_nanodollars": 150000000, "event_count": 85}
```

### curl

```bash
# Ingest with request/response bodies
curl -X POST http://localhost:8080/v1/events \
  -H "Authorization: Bearer sk-your-key" \
  -H "Content-Type: application/json" \
  -d '{
    "model": "claude-sonnet-4-20250514",
    "provider": "anthropic",
    "usage": {
      "input_tokens": 2000,
      "output_tokens": 1000,
      "cache_read_input_tokens": 500
    },
    "latency": { "ttft_ms": 45, "total_ms": 800 },
    "http_status": 200,
    "user_id": "alice",
    "flags": { "streaming": true, "reasoning": true },
    "request_body": {
      "model": "claude-sonnet-4-20250514",
      "messages": [{"role": "user", "content": "Explain quantum computing"}]
    },
    "response_body": {
      "content": [{"type": "text", "text": "Quantum computing uses..."}]
    }
  }'

# Query events for a user
curl "http://localhost:8080/v1/events?user_id=alice&limit=10" \
  -H "Authorization: Bearer sk-your-key"

# Get daily rollups
curl "http://localhost:8080/v1/rollups?from=1700000000000000000&to=1710000000000000000&user_id=alice" \
  -H "Authorization: Bearer sk-your-key"

# Get stats grouped by model
curl "http://localhost:8080/v1/stats?from=1700000000000000000&to=1710000000000000000&group_by=model" \
  -H "Authorization: Bearer sk-your-key"
```

### Node.js (fetch)

```javascript
const KEPLOR_URL = "http://localhost:8080";
const API_KEY = "sk-your-key";

const headers = {
  "Authorization": `Bearer ${API_KEY}`,
  "Content-Type": "application/json",
};

// Ingest after an LLM call
async function logLlmCall(model, provider, usage, latencyMs) {
  const resp = await fetch(`${KEPLOR_URL}/v1/events`, {
    method: "POST",
    headers,
    body: JSON.stringify({
      model,
      provider,
      usage,
      latency: { total_ms: latencyMs },
      http_status: 200,
      source: "my-node-app",
    }),
  });
  return resp.json();
}

// Example usage
const result = await logLlmCall("gpt-4o", "openai", {
  input_tokens: 1200,
  output_tokens: 600,
}, 350);

console.log(`Event ${result.id} cost: $${(result.cost_nanodollars / 1e9).toFixed(6)}`);
```

### LiteLLM Callback Integration

If you use LiteLLM as your gateway, configure a custom callback to forward events:

```python
import litellm
import requests

KEPLOR_URL = "http://localhost:8080"

def keplor_callback(kwargs, completion_response, start_time, end_time):
    latency_ms = int((end_time - start_time).total_seconds() * 1000)
    usage = completion_response.get("usage", {})

    requests.post(f"{KEPLOR_URL}/v1/events", json={
        "model": kwargs.get("model", ""),
        "provider": kwargs.get("custom_llm_provider", "openai"),
        "usage": {
            "input_tokens": usage.get("prompt_tokens", 0),
            "output_tokens": usage.get("completion_tokens", 0),
        },
        "latency": {"total_ms": latency_ms},
        "http_status": 200,
        "user_id": kwargs.get("user", None),
        "source": "litellm",
    })

litellm.success_callback = [keplor_callback]
```

---

## Prometheus Metrics

Keplor exposes the following metrics at `GET /metrics`:

| Metric | Type | Labels | Description |
|--------|------|--------|-------------|
| `keplor_events_ingested_total` | counter | `provider` | Total events ingested |
| `keplor_events_errors_total` | counter | `stage` | Ingestion errors by stage (`validation`, `store`, `queue_full`) |
| `keplor_ingest_duration_seconds` | histogram | | End-to-end ingest latency (p50/p95/p99) |
| `keplor_batch_flushes_total` | counter | | Batch flush operations completed |
| `keplor_batch_events_flushed_total` | counter | | Total events written to SQLite |
| `keplor_batch_flush_errors_total` | counter | | Batch flush failures |
| `keplor_auth_successes_total` | counter | | Successful auth attempts |
| `keplor_auth_failures_total` | counter | `reason` | Failed auth attempts (`missing`, `invalid`) |

---

## Operations

### Automated Garbage Collection

Keplor supports **tiered retention** — different API keys can have different retention periods. Configure tiers in `[retention]`:

```toml
[retention]
default_tier = "free"

[[retention.tiers]]
name = "free"
days = 7

[[retention.tiers]]
name = "pro"
days = 90

[[retention.tiers]]
name = "team"
days = 180
```

GC runs every `storage.gc_interval_secs` (default: 3600 = 1 hour), one pass per tier. Each pass:
- Deletes events of that tier older than the tier's retention window
- Decrements blob refcounts and removes orphaned blobs
- If an external blob store (S3/R2) is configured, deletes orphaned blobs from external storage

Set `days = 0` on a tier to keep events forever. Set `storage.gc_interval_secs = 0` to disable automated GC entirely (you can still run `keplor gc --older-than-days N` manually).

**Legacy mode:** If no `[retention]` section is present, `storage.retention_days` is used as a global fallback.

### WAL Checkpointing

SQLite WAL mode can accumulate a large write-ahead log under sustained write load. Keplor automatically runs `PRAGMA wal_checkpoint(TRUNCATE)` every `storage.wal_checkpoint_secs` (default: 300 seconds) and on shutdown.

### Monitoring

For production, point your Prometheus scraper at `GET /metrics` and set up alerts on:
- `rate(keplor_events_errors_total[5m]) > 0` — ingestion failures
- `rate(keplor_batch_flush_errors_total[5m]) > 0` — storage write failures
- `rate(keplor_auth_failures_total[5m]) > 10` — auth brute-force attempts
- `histogram_quantile(0.99, keplor_ingest_duration_seconds)` — p99 latency
