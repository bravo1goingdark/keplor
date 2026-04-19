# Architecture

Keplor is an LLM logs ingestion and cost-accounting server. External systems POST pre-parsed LLM event data; Keplor validates, prices, compresses, and stores it.

## Crate layout

```
keplor-core       Zero-dep types: Event, Provider, Usage, Cost, error enums.
keplor-pricing    LiteLLM pricing catalog, cost computation (nanodollars).
keplor-store      SQLite storage, batch writer, GC, rollups, S3/R2 archival.
keplor-server     HTTP server, ingestion pipeline, auth, rate limiting, CORS.
keplor-cli        The `keplor` binary (run, migrate, query, stats, gc, rollup, archive).
xtask             Build automation (refresh-catalog, mem-audit).
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
│    4. build LlmEvent                              │
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
│                                                  │
│  Archiver (optional, every 1h):                  │
│    - Serializes old events to JSONL + zstd        │
│    - Uploads to S3/R2, deletes from SQLite        │
└──────────────────────────────────────────────────┘
```

## Database schema (SQLite)

| Table | Purpose |
|-------|---------|
| `schema_version` | Migration versioning |
| `llm_events` | Fact table — one row per ingested event (44 columns) |
| `daily_rollups` | Pre-aggregated daily cost/usage summaries |
| `archive_manifests` | Tracks archived JSONL files in S3/R2 (optional) |

9 indices cover timestamp, user/key/model/provider/source/tier lookups.

## Key design decisions

**SQLite + WAL mode**: Zero-dep default. Read/write connection split (1 writer, 4 readers) prevents writer starvation. WAL checkpoints run every 300s + on shutdown.

**Batch writer**: Events queue in an mpsc channel (capacity 32768, configurable). Background task flushes in bulk transactions. Single-event endpoint (`POST /v1/events`) awaits flush confirmation. Batch endpoint (`POST /v1/events/batch`) is fire-and-forget by default; set `X-Keplor-Durable: true` for confirmed writes.

**Event archival**: Old events are archived to S3/R2 as zstd-compressed JSONL files, partitioned by (user_id, day). Daily rollups are preserved in SQLite. Archive manifests track what was uploaded. Runs every hour by default (configurable via `archive_interval_secs`) with per-chunk error isolation.

**Cost in nanodollars (int64)**: Avoids floating-point rounding. 1 nanodollar = 10^-9 USD. Max representable: ~$9.2 billion.

**Health/metrics bypass connection limits**: The `/health` and `/metrics` endpoints are not subject to the `max_connections` concurrency limit, so observability remains accessible under full saturation.

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
