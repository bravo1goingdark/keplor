# Phase 3 — Storage and compression

**Status:** not started
**Depends on:** phase 1
**Unlocks:** phases 4, 6, 8

## Goal

Durable local storage of `LlmEvent` records and their request/response bodies, with content-addressed deduplication and zstd compression (dict training added in phase 8).

## Prompt

Implement `keplor-store`.

### 1. SQLite schema

Migrations as const `&str` arrays, applied in order on open.

```sql
-- 0001_init.sql
CREATE TABLE schema_version(version INTEGER PRIMARY KEY, applied_at INTEGER);

CREATE TABLE llm_events (
  id BLOB PRIMARY KEY,                   -- ULID 16 bytes
  ts_ns INTEGER NOT NULL,
  user_id TEXT, api_key_id TEXT, org_id TEXT, project_id TEXT, route_id TEXT,
  provider TEXT NOT NULL, model TEXT NOT NULL, model_family TEXT,
  endpoint TEXT NOT NULL, method TEXT NOT NULL, http_status INTEGER,
  input_tokens INTEGER NOT NULL DEFAULT 0,
  output_tokens INTEGER NOT NULL DEFAULT 0,
  cache_read_input_tokens INTEGER NOT NULL DEFAULT 0,
  cache_creation_input_tokens INTEGER NOT NULL DEFAULT 0,
  reasoning_tokens INTEGER NOT NULL DEFAULT 0,
  audio_input_tokens INTEGER NOT NULL DEFAULT 0,
  audio_output_tokens INTEGER NOT NULL DEFAULT 0,
  image_tokens INTEGER NOT NULL DEFAULT 0,
  tool_use_tokens INTEGER NOT NULL DEFAULT 0,
  cost_nanodollars INTEGER NOT NULL DEFAULT 0,
  latency_ttft_ms INTEGER, latency_total_ms INTEGER, time_to_close_ms INTEGER,
  streaming INTEGER NOT NULL, tool_calls INTEGER NOT NULL,
  reasoning INTEGER NOT NULL, stream_incomplete INTEGER NOT NULL,
  error_type TEXT, error_message TEXT,
  request_sha256 BLOB NOT NULL, response_sha256 BLOB NOT NULL,
  request_blob_id BLOB, response_blob_id BLOB,
  client_ip TEXT, user_agent TEXT, request_id TEXT, trace_id TEXT
) STRICT;

CREATE INDEX idx_events_ts ON llm_events(ts_ns);
CREATE INDEX idx_events_user_ts ON llm_events(user_id, ts_ns);
CREATE INDEX idx_events_key_ts ON llm_events(api_key_id, ts_ns);
CREATE INDEX idx_events_model_ts ON llm_events(model, ts_ns);

CREATE TABLE payload_blobs (
  sha256 BLOB PRIMARY KEY,
  component_type TEXT NOT NULL,          -- 'system_prompt'|'tools'|'messages'|'response'|'raw'
  provider TEXT NOT NULL,
  compression TEXT NOT NULL,             -- 'zstd_raw'|'zstd_dict:<dict_id>'
  dict_id TEXT,
  uncompressed_size INTEGER NOT NULL,
  compressed_size INTEGER NOT NULL,
  refcount INTEGER NOT NULL DEFAULT 1,
  hit_count INTEGER NOT NULL DEFAULT 0,
  data BLOB NOT NULL,
  first_seen_at INTEGER NOT NULL
) STRICT;

CREATE TABLE zstd_dicts (
  id TEXT PRIMARY KEY,
  provider TEXT NOT NULL,
  component_type TEXT NOT NULL,
  sample_count INTEGER NOT NULL,
  created_at INTEGER NOT NULL,
  data BLOB NOT NULL
) STRICT;

CREATE TABLE daily_rollups (
  day INTEGER NOT NULL, user_id TEXT, api_key_id TEXT, model TEXT,
  event_count INTEGER, input_tokens INTEGER, output_tokens INTEGER,
  cost_nanodollars INTEGER,
  PRIMARY KEY(day, user_id, api_key_id, model)
) STRICT;
```

Apply pragmas on open:
```
PRAGMA journal_mode=WAL;
PRAGMA synchronous=NORMAL;
PRAGMA mmap_size=268435456;
PRAGMA busy_timeout=5000;
```

### 2. Store API

`Store` struct owns a `rusqlite::Connection` behind a `Mutex` (or a `deadpool` pool if bench shows contention). Provides:

- `append_event(event: LlmEvent, req_body: Bytes, resp_body: Bytes) -> Result<EventId>`
  Internally: split payload into components (see §3), sha256 each, upsert into `payload_blobs` with refcount++, insert `llm_events` row.
- `get_event(id: EventId) -> Result<Option<LlmEvent>>`
- `query(filter: EventFilter, limit: u32, cursor: Option<Cursor>) -> Result<Vec<LlmEvent>>`
- `rollup_day(day: NaiveDate) -> Result<()>`
- `gc_expired(older_than: DateTime<Utc>) -> Result<GcStats>` — deletes events + decrements refcount; blobs with refcount=0 are removed.

### 3. Payload-component splitting (`components.rs`)

For each provider, extract:
- `system_prompt` (single component if present)
- `tools` / tool-schema array (single component per request)
- `messages` (single component — do NOT deduplicate per-message; overfitting)
- `response_text` / `response_content` (single component)

Return `Vec<(ComponentType, Bytes)>`. Reuse components across requests via sha256 — that's where the biggest wins come from.

### 4. Compression (`compress.rs`)

- `ZstdCoder` wraps `zstd::bulk::{Compressor, Decompressor}`.
- Default: zstd level 3 with no dict (fallback path).
- Per `(provider, component_type)` trained dict: loaded from `zstd_dicts` table at startup into an `Arc<HashMap<DictKey, Arc<EncoderDictionary>>>`. (Dict training itself is Phase 8 — here, infrastructure only.)
- Never delete old dicts — blobs reference them by id forever.

### 5. Backup + migrations

- `Store::migrate()` walks versioned SQL files, records in `schema_version`.
- `keplor db vacuum`, `keplor db backup <path>`, `keplor db restore <path>` CLI hooks (implemented in phase 6).

## Acceptance criteria

- [ ] Round-trip: `append_event` → `get_event`, bytes exact
- [ ] Dedup: two events sharing a system_prompt produce 1 blob with refcount=2
- [ ] GC: deleting both events drops the blob
- [ ] Compression-ratio smoke test: 1000 synthetic OpenAI chat completions → raw NDJSON vs our store. Assert compressed size < 5% of raw (>20× compression)
- [ ] Concurrent writes from 8 tasks don't deadlock (tokio test)
- [ ] `cargo test -p keplor-store` green
- [ ] Report the compression ratio achieved in `docs/progress.md`
