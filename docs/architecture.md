# Architecture

Keplor is an LLM logs ingestion and cost-accounting server. External systems POST pre-parsed LLM event data; Keplor validates, prices, compresses, and stores it.

## Crate layout

```
keplor-core       Zero-dep types: Event, Provider, Usage, Cost, error enums.
keplor-pricing    LiteLLM pricing catalog, cost computation (nanodollars).
keplor-store      SQLite storage, zstd compression, batch writer, GC, rollups.
keplor-server     HTTP server, ingestion pipeline, auth, rate limiting, CORS.
keplor-cli        The `keplor` binary (run, migrate, query, stats, gc, rollup).
xtask             Build automation (refresh-catalog, size-audit).
```

Dependency flow: `cli → server → {store, pricing} → core`.

## Data flow

```
Client (app/gateway/SDK)
  │
  │ POST /v1/events  { model, provider, usage, ... }
  ▼
┌──────────────────────────────────────────────────┐
│  keplor-server                                   │
│                                                  │
│  request_id → auth → rate_limit → pipeline       │
│                                                  │
│  pipeline:                                       │
│    1. validate (field constraints, size limits)   │
│    2. normalize (provider enum, model lowercase)  │
│    3. compute cost (LiteLLM catalog lookup)       │
│    4. build LlmEvent + SHA-256 body hashes        │
│    5. write to BatchWriter channel                │
└──────────────────────────────────────────────────┘
  │
  ▼
┌──────────────────────────────────────────────────┐
│  keplor-store                                    │
│                                                  │
│  BatchWriter (background task):                  │
│    - Accumulates events in memory (up to 256)    │
│    - Flushes every 50ms or when batch is full    │
│    - Bulk INSERT in single SQLite transaction     │
│    - Content-addressed blob storage (dedup)       │
│    - zstd compression with trained dictionaries   │
└──────────────────────────────────────────────────┘
```

## Database schema (SQLite)

| Table | Purpose |
|-------|---------|
| `schema_version` | Migration versioning |
| `llm_events` | Fact table — one row per ingested event (23 columns) |
| `payload_blobs` | Content-addressed blob store (SHA-256 keyed) |
| `event_components` | Junction: event ↔ blob (request, response, metadata) |
| `zstd_dicts` | Trained compression dictionaries per (provider, component_type) |
| `daily_rollups` | Pre-aggregated daily cost/usage summaries |

9 indices cover timestamp, user/key/model/provider/source lookups.

## Key design decisions

**SQLite + WAL mode**: Zero-dep default. Read/write connection split (1 writer, 4 readers) prevents writer starvation. WAL checkpoints run every 300s + on shutdown.

**Batch writer**: Events queue in an mpsc channel (capacity 8192). Background task flushes in bulk transactions. Single-event endpoint (`POST /v1/events`) awaits flush confirmation. Batch endpoint (`POST /v1/events/batch`) is fire-and-forget by default; set `X-Keplor-Durable: true` for confirmed writes.

**Content-addressed blobs**: Request/response bodies are SHA-256 hashed. Identical payloads (e.g., repeated system prompts) are stored once with a reference count.

**zstd with trained dictionaries**: Dictionaries are trained per (provider, component_type) to exploit the repetitive structure of LLM JSON. Targets 30-80x compression on conversational traffic.

**Cost in nanodollars (int64)**: Avoids floating-point rounding. 1 nanodollar = 10^-9 USD. Max representable: ~$9.2 billion.

## API endpoints

| Method | Path | Auth | Description |
|--------|------|------|-------------|
| POST | `/v1/events` | Yes | Ingest single event (durable) |
| POST | `/v1/events/batch` | Yes | Ingest batch (fire-and-forget or durable via `X-Keplor-Durable`) |
| GET | `/v1/events` | Yes | Query events with filters + cursor pagination |
| GET | `/v1/events/export` | Yes | Stream all matching events as JSON Lines |
| DELETE | `/v1/events/{id}` | Yes | Delete a single event |
| DELETE | `/v1/events?older_than_days=N` | Yes | Bulk delete old events |
| GET | `/v1/quota` | Yes | Real-time cost/count for a user or key |
| GET | `/v1/rollups` | Yes | Pre-aggregated daily rollup rows (paginated) |
| GET | `/v1/stats` | Yes | Period statistics, optionally grouped by model (paginated) |
| GET | `/health` | No | Liveness probe (DB + queue status) |
| GET | `/metrics` | No | Prometheus text format |
