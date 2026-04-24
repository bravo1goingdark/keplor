# Phase A — KeplorDB cutover (foundation)

Status: **partial — foundation landed, integration pending.** Dated 2026-04-25.

## Scope of this phase

Full replacement of SQLite by KeplorDB as the storage backend for
`llm_events` + `daily_rollups` + archive manifests. This retrospective
covers the first four sub-phases; the remaining three are open tasks.

## What landed

### Sub-phase 0a — `Engine::append_durable` (keplordb repo)

KeplorDB now has per-call fsync variants suitable for keplor's durable
ingest contract.

- `Wal::append_durable(&LogEvent) -> (shard_idx, is_full)` — writes the
  event, fsync's the shard WAL, returns.
- `Wal::append_batch_durable(&[LogEvent]) -> Vec<(shard_idx, is_full)>` —
  single fsync per batch.
- `Engine::append_durable` and `Engine::append_batch_durable` expose
  the same shape at the public API level.
- Fixed a pre-existing bug in `WalWriter::create`: the 8-byte WAL
  header was buffered but never flushed, so an untouched shard's file
  on disk was empty and broke `replay_all_wals` with `UnexpectedEof`.
  The new `append_durable` path triggered this because it only writes
  to the one shard that received the event.

Tests added: `append_durable_persists_without_sync_interval`,
`append_batch_durable_persists_without_sync_interval`,
`append_durable_fsyncs_each_write`,
`append_batch_durable_fsyncs_once_per_batch`. 265 keplordb lib tests
pass; `cargo clippy -- -D warnings` clean.

### Sub-phase 0b — `Engine::delete_events` batch tombstone (keplordb repo)

`delete_event(id)` clones the entire tombstone set per id —
O(n × |tombstones|) for the archive flow that deletes thousands of ids
in one go.

- `Engine::delete_events(&[&str])` builds the new set in one pass and
  appends all ids to the tombstone journal under a single `fsync_data`
  call.

Tests: `delete_events_batch_tombstones_all_ids`,
`delete_events_empty_slice_is_noop`.

### Sub-phase 1 — mapping module (`crates/keplor-store/src/mapping.rs`)

The `LlmEvent ↔ keplordb::LogEvent` schema, declared once as compile-
time constants that must not be reordered (they're part of the
on-disk segment format).

Layout:

- `D = 14` dims: `user_id, api_key_id, model, provider, source,
  org_id, project_id, route_id, model_family, endpoint, tier,
  error_type, user_tag, session_tag`. `user_tag`/`session_tag` are
  **promoted from JSON-path filters to first-class dims** so tag
  queries skip segments instead of scanning `metadata_json`. This is a
  wire-compatible change (tags are still sent via `metadata`) but
  means events ingested before the cutover lose tag-indexed queries.
- `C = 13` counters: all 11 `Usage` fields + `time_to_close_ms` +
  `is_error (0/1)`. The `is_error` slot makes
  `aggregate_stats.error_count` fall out as a saturating sum.
- `L = 8` labels: `error_json, request_id, trace_id_hex, client_ip,
  user_agent, metadata_json, method, provider_variant`. The last slot
  stores the `Provider::OpenAICompatible { base_url }` payload.
- Flags (`u16`): 6 domain bits (matching keplor-core's `EventFlags`) +
  4 presence bits (`has_ttft`, `has_http_status`, `has_error`,
  `has_close_ms`) so `Option<T>` scalars survive the round trip.

Error encoding: `ProviderError` uses `#[serde(tag = "kind")]` with
tuple variants (`InvalidRequest(String)`), which `serde_json` cannot
serialise into a map. A hand-written compact JSON format
(`{"t":"...","m":"...","r":30,"l":128000,"s":418}`) round-trips every
variant **losslessly** — unlike the existing SQLite store, which lost
`retry_after`, `ContextLengthExceeded.limit`, and `Other.status`.

Dropped from `LlmEvent` storage: `request_sha256` /
`response_sha256` (already dead per `event.rs:62`); `ingested_at` (use
the ULID's time component instead).

`SCHEMA_ID = 1` is written into every segment header; KeplorDB
refuses to open a segment with a mismatched id.

18 unit tests cover full round-trip, every provider variant, every
error variant, `Option<T>` Some(0)/None distinction, trace-id hex
handling, filter mapping, and compile-time bounds (`D ≤ 256`,
`C ≤ 64`, `L ≤ 64`).

### Sub-phase 2 — `KdbStore` (`crates/keplor-store/src/kdb_store.rs`)

The new store. One `keplordb::Engine<14, 13, 8>` per retention tier,
spawned under `{data_dir}/tier={tier}/`. Cross-tier reads fan out and
merge by `ts_ns` desc; per-tier GC drops whole segments without
touching other tiers, which solves the mismatch between KeplorDB's
segment-granular GC and keplor's per-tier retention.

Architecture:

- `engines: ArcSwap<HashMap<SmolStr, Arc<Engine>>>` — lock-free reads
  on the hot path. Dynamic tiers are lazily inserted under a
  `Mutex<()>` to prevent double-create.
- `ManifestStore` sidecar at `{data_dir}/archive_manifests.jsonl`:
  append-only JSONL + in-memory `BTreeMap<(user_id, day),
  Vec<ArchiveManifest>>`. Tolerates truncated trailing line on reopen
  for crash safety.
- Configurable per-tier eager initialisation
  (`eager_tiers: Vec<SmolStr>`, default `["free", "pro", "team"]`).

API parity: all 27 methods of the existing SQLite `Store` are
implemented on `KdbStore` (ingest, query, summary, quota, rollups,
stats, GC, archive, health, diagnostics).

Read-visibility semantics: KeplorDB queries only see events that have
rotated into segments. Events in the active WAL are durable on disk
but not queryable until rotation. In production, `BatchWriter` flushes
every 50 ms / 256 events, so read lag is capped at that window.
Callers needing write-then-read-immediately must call
`wal_checkpoint` explicitly. Documented at the top of `kdb_store.rs`.

13 unit tests (in addition to 5 manifest-sidecar tests): open creates
eager tiers, dynamic tier is lazily created, append + get round-trip,
cross-tier query merge with limit + ordering, cross-tier quota sum,
per-tier GC isolation, batch tombstone idempotency, health probe.

## What's green

- `cargo test -p keplor-store --lib` → 40 passed, 0 failed.
- `cargo test -p keplordb --lib` → 265 passed, 0 failed.
- `cargo clippy --all-targets -- -D warnings` clean on both crates.
- `cargo fmt --check` clean.
- `cargo build` across the whole keplor workspace clean.

## What's still open

Three sub-phases remain before the cutover is user-visible.

- **Sub-phase 3 — migration CLI.** `keplor migrate-from-sqlite
  --source keplor.db --dest ./keplor_data/`. Reads SQLite in 10k-event
  chunks, uses `mapping::to_log_event` + `KdbStore::append_batch`,
  copies `archive_manifests` rows into the JSONL sidecar. Resumable
  via a checkpoint file so a crash mid-migration doesn't force a full
  restart.
- **Sub-phase 4 — rewire call sites.** `pipeline.rs`, `routes.rs`,
  `server.rs`, `config.rs`, and the CLI commands all hold `Arc<Store>`
  (SQLite) today. The existing `Store` API shape is close enough to
  `KdbStore`'s that the rewire is mostly s/Store/KdbStore/ + config
  changes (`path = "keplor.db"` → `data_dir = "./keplor_data/"`).
  Remove the SQL `migrate` subcommand, keep a lightweight
  `verify-schema` that checks `SCHEMA_ID`.
- **Sub-phase 5 — tests + benchmarks.** Retarget
  `tests/http_integration.rs`, `keplor-store/tests/integration.rs`,
  `benches/pipeline_bench.rs`, `benches/store_bench.rs`. Add per-tier
  GC integration test, cross-tier aggregate parity test, archive
  round-trip test, crash recovery test. Compare benchmark numbers
  against the SQLite baseline.
- **Sub-phase 6 — docs + workspace dep swap.** Rewrite the Storage
  section of `docs/architecture.md`. Document per-tier data dir
  layout in `docs/operations.md`. Remove `rusqlite` from
  `crates/keplor-store/Cargo.toml`; delete
  `crates/keplor-store/src/{store.rs, migrations.rs,
  stored_event.rs, filter.rs SQL parts}` once nothing references
  them. Add `keplordb` to workspace deps (currently added
  crate-local).

## Risks + watch-outs for the next phase

1. Read visibility lag. If any endpoint expects immediate
   write-then-read consistency (probably `POST /v1/events` +
   subsequent `GET /v1/events`), the `BatchWriter` must flush
   aggressively or the route must force `wal_checkpoint`. Audit this
   during sub-phase 4.

2. `ProviderError` round-trip is now lossless for the new store.
   Existing SQLite data that migrates through sub-phase 3 will still
   lack the `retry_after`/`context_limit`/`Other.status` fields —
   nothing to do except note it.

3. KeplorDB is pinned to a local path
   (`../../../keplordb/crates/keplordb`) in
   `crates/keplor-store/Cargo.toml`. Once KeplorDB publishes 0.2.0
   with `append_durable` and batch tombstone, swap to a git ref or
   crates.io version. Until then, keplor is yoked to whatever SHA
   exists on disk.

4. Workspace-level dep swap (removing `rusqlite` from
   `workspace.dependencies`) needs user confirmation per CLAUDE.md §
   File ownership.

## Commit hygiene

No commits have been created in this session. The keplordb repo has
pre-existing local deletions of `PRODUCTION.md` and `plan.md` that are
not part of this work — when committing, stage only the files I
touched:

- keplordb: `crates/keplordb/src/write/wal.rs`,
  `crates/keplordb/src/write/recovery.rs`,
  `crates/keplordb/src/engine.rs`.
- keplor: `crates/keplor-store/Cargo.toml`,
  `crates/keplor-store/src/lib.rs`,
  `crates/keplor-store/src/error.rs`,
  `crates/keplor-store/src/mapping.rs`,
  `crates/keplor-store/src/kdb_store.rs`,
  `crates/keplor-store/src/kdb_store/manifests.rs`, and this file.

Suggested commit split:

1. `phase-A: keplordb: add append_durable + batch tombstone` (in
   keplordb repo).
2. `phase-A: keplor-store: LlmEvent ↔ LogEvent mapping module` (in
   keplor repo).
3. `phase-A: keplor-store: KdbStore with per-tier engines + manifest
   sidecar` (in keplor repo).
