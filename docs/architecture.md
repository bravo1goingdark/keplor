# Architecture

Keplor is an LLM logs ingestion and cost-accounting server. External systems POST pre-parsed LLM event data; Keplor validates, prices, compresses, and stores it.

## Crate layout

```
keplor-core       Zero-dep types: Event, Provider, Usage, Cost, error enums.
keplor-pricing    LiteLLM pricing catalog, cost computation (nanodollars).
keplor-store      KeplorDB-backed event store, batch writer, GC, rollups, S3/R2 archival.
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
│    - append_batch + wal_checkpoint per flush     │
│      (KeplorDB queries only see rotated segs)    │
│                                                  │
│  Archiver (optional, every 1h):                  │
│    - Serializes old events to JSONL + zstd       │
│    - Uploads to S3/R2, then GCs the segments     │
└──────────────────────────────────────────────────┘
```

## Storage (KeplorDB)

Events are stored in **KeplorDB**, a custom append-only columnar log
engine (sibling repo, `crates/keplordb`). The on-disk layout per data
directory:

```
keplor_data/
  free/   pro/   team/   <tier>/...      # one Engine per retention tier
    wal-0000 ... wal-N                   # active write-ahead segments (sharded)
    seg-XXXX.kdb                         # rotated immutable segments
  manifests.jsonl                        # archive manifest sidecar
```

Each tier is an `Engine<14, 13, 8>` (14 dims, 13 counters, 8 labels,
`SCHEMA_ID = 1`). The schema is defined in `keplor-store/src/mapping.rs`:

| | Used as |
|--|--|
| **Dims (D=14)** | `user_id`, `api_key_id`, `org_id`, `project_id`, `route_id`, `provider`, `model`, `model_family`, `endpoint`, `method`, `source`, `tier`, `user_tag`, `session_tag` |
| **Counters (C=13)** | input/output/cache-read/cache-creation/reasoning/audio-in/audio-out/image/tool-use tokens, `cost_nanodollars`, `latency_total_ms`, `latency_ttft_ms`, `is_error` |
| **Labels (L=8)** | `endpoint`, `method`, `request_id`, `trace_id_hex`, `client_ip`, `user_agent`, `error_blob`, `metadata_json` |

Per-tier routing (`KdbStore::engines: ArcSwap<HashMap<SmolStr, Arc<Engine>>>`)
exists because KeplorDB's GC is segment-granular but keplor's retention
is per-tier — keeping each tier in its own Engine lets segment GC and
retention policies stay aligned. The `eager_tiers` config (default
`["free", "pro", "team"]`) pre-creates engines at startup; unknown tiers
are created lazily on first write.

Archive manifests live in a JSONL sidecar (`manifests.jsonl`) plus an
in-memory `BTreeMap<(user_id, day), Vec<ArchiveManifest>>` for fast
lookup; KeplorDB itself doesn't carry secondary tables.

**Read visibility**: `KdbStore::query` only sees rotated segments —
events still in the active WAL are invisible. The BatchWriter calls
`wal_checkpoint()` after every flush so HTTP follow-up reads are
consistent with the write that just succeeded.

## Key design decisions

**KeplorDB columnar log**: Append-only log with sharded WALs, periodic
rotation to immutable `.kdb` segments, and segment-level GC. Per-tier
Engine isolation; no rusqlite, no SQL planner, no per-row B-tree pages.
At idle, each `BatchWriter` flush produces one segment per tier
(~1200/min total at the default 50 ms cadence), reclaimed by retention GC.

**Batch writer**: Events queue in an mpsc channel (capacity 32768, configurable). Background task flushes in bulk via `Engine::append_batch` followed by `wal_checkpoint`. Single-event endpoint (`POST /v1/events`) awaits flush confirmation. Batch endpoint (`POST /v1/events/batch`) is fire-and-forget by default; set `X-Keplor-Durable: true` for confirmed writes.

**Event archival**: Old events are archived to S3/R2 as zstd-compressed JSONL files, partitioned by (user_id, day). Daily rollup queries replay against the JSONL sidecar for archived ranges. Manifests track what was uploaded. Runs every hour by default (configurable via `archive_interval_secs`) with per-chunk error isolation.

**Legacy SQLite migration**: The previous SQLite backend is retained as
a read-only migration source under the `migrate-from-sqlite` Cargo
feature on `keplor-store` and `keplor-cli`. Default release builds
**omit `rusqlite` entirely** (~2 MB binary saving). The
`keplor migrate-from-sqlite` subcommand opens both stores, walks the
SQLite DB in chunks, converts each event via the mapping module, and
writes to the per-tier KeplorDB engines with a resumable on-disk
checkpoint.

**Cost in nanodollars (int64)**: Avoids floating-point rounding. 1 nanodollar = 10^-9 USD. Max representable: ~$9.2 billion.

**Health/metrics bypass connection limits**: The `/health` and `/metrics` endpoints are not subject to the `max_connections` concurrency limit, so observability remains accessible under full saturation.

## API endpoints

| Method | Path | Auth | Description |
|--------|------|------|-------------|
| POST | `/v1/events` | Yes | Ingest single event (durable) |
| POST | `/v1/events/batch` | Yes | Ingest batch (fire-and-forget or durable via `X-Keplor-Durable`) |
| GET | `/v1/events` | Yes | Query events with filters + cursor pagination. Optional `?include_archived=true` opts into merging archived (S3) events into the response (uncached; one round trip per overlapping manifest). |
| GET | `/v1/events/export` | Yes | Stream all matching events as JSON Lines |
| DELETE | `/v1/events/{id}` | Yes | Delete a single event |
| DELETE | `/v1/events?older_than_days=N` | Yes | Bulk delete old events |
| GET | `/v1/quota` | Yes | Real-time cost/count for a user or key |
| GET | `/v1/rollups` | Yes | Pre-aggregated daily rollup rows (paginated) |
| GET | `/v1/stats` | Yes | Period statistics, optionally grouped by model (paginated) |
| GET | `/health` | No | Liveness probe (DB + queue status) |
| GET | `/metrics` | No | Prometheus text format |
